//! Report generation and scheduling: which reports the shown tab and the
//! schedule need (freshness, the retry backoff, the Weekly Summary's wait
//! after an edit), streaming a report and storing it when it ends, the Week
//! tab's report panel, and catching up after anything that may make a
//! report due.

use crate::{Message, Ready};
use fortnight_app::top_bar::Tab;
use fortnight_app::views::report_panel::{self, ReportPanelData, ReportSlot};
use fortnight_core::domain::{Report, ReportKey, ReportKind, ReportStatus, WeekRef};
use fortnight_core::fallback::PlanRoute;
use fortnight_core::llm::{ChatEvent, ChatResponse, LlmError};
use fortnight_core::report::{self, Prepared};
use fortnight_core::store::Store;
use fortnight_core::week::due_reports;
use iced::Task;
use jiff::civil::Date;
use jiff::{SignedDuration, Timestamp};
use std::collections::HashMap;

/// A failed automatic generation is retried after this long, doubling after
/// each further failure in a row, up to `RETRY_MAX_MINUTES`.
const RETRY_AFTER_MINUTES: i64 = 10;
const RETRY_MAX_MINUTES: i64 = 24 * 60;

/// After a change on the Week tab, the Weekly Summary waits this long for
/// more changes (each one restarts the wait) before it is redone.
const SUMMARY_WAIT_SECS: i64 = 30;

/// A report being generated: what was asked, and the text so far.
struct Generation {
    prepared: Prepared,
    text: String,
    /// Where the request went (recommended tasks: the decision model, if
    /// any, falling back to the main endpoint), for the report's model and
    /// host.
    route: PlanRoute,
    /// Started by the schedule: badge the Week tab when it is ready, and
    /// never aborted for being out of view.
    scheduled: bool,
    /// Aborts the request (Lock stops requests that hold the key).
    abort: iced::task::Handle,
    id: u64,
}

/// The state of report generation.
#[derive(Default)]
pub(crate) struct Reports {
    generating: HashMap<ReportKey, Generation>,
    last_attempt: HashMap<ReportKey, Timestamp>,
    /// Failures in a row per report, for the retry backoff.
    failures: HashMap<ReportKey, u32>,
    /// When each prompt was last changed in Settings this session: versions
    /// made before then are out of date.
    prompt_changed: HashMap<ReportKind, Timestamp>,
    /// Identifies a generation, so a late message from an earlier stream for
    /// the same report can't finish a newer one.
    next_generation: u64,
    /// The Weekly Summary isn't redone before this: set by each change made
    /// on the Week tab (`SUMMARY_WAIT_SECS`), so a run of edits makes one
    /// new summary, not one each.
    summary_wait: Option<Timestamp>,
}

impl Reports {
    /// Whether `key` is being generated.
    pub(super) fn is_generating(&self, key: &ReportKey) -> bool {
        self.generating.contains_key(key)
    }

    /// Aborts every generation (Lock); they are dropped, not stored.
    pub(super) fn abort_all(&mut self) {
        for (_, g) in self.generating.drain() {
            g.abort.abort();
        }
    }

    /// The prompt of `kind` was changed in Settings `at`.
    pub(super) fn note_prompt_change(&mut self, kind: ReportKind, at: Timestamp) {
        self.prompt_changed.insert(kind, at);
    }

    /// Whether the Weekly Summary has a wait after an edit set.
    pub(crate) fn has_summary_wait(&self) -> bool {
        self.summary_wait.is_some()
    }

    /// Starts (or restarts) the Weekly Summary's wait after an edit.
    pub(crate) fn start_summary_wait(&mut self) {
        self.summary_wait = Some(Timestamp::now() + SignedDuration::from_secs(SUMMARY_WAIT_SECS));
    }

    /// Whether the Weekly Summary is still waiting after a Week-tab change.
    fn summary_waiting(&self) -> bool {
        self.summary_wait.is_some_and(|at| Timestamp::now() < at)
    }

    /// A recent attempt at `key` failed and its retry wait hasn't passed
    /// (`backoff_active`).
    fn backing_off(&self, key: ReportKey) -> bool {
        backoff_active(
            self.failures.get(&key).copied().unwrap_or(0),
            self.last_attempt.get(&key).copied(),
            Timestamp::now(),
        )
    }

    /// Counts a failed attempt at `key` now, for the backoff.
    fn record_failure(&mut self, key: ReportKey) {
        *self.failures.entry(key).or_insert(0) += 1;
        self.last_attempt.insert(key, Timestamp::now());
    }
}

impl Ready {
    /// A streamed chunk arrived for `key`: shows the text so far in its
    /// Week panel slot, if that week is showing, without a full refresh.
    fn show_streaming(&mut self, key: ReportKey) {
        if self.tab != Tab::Week {
            return;
        }
        let Some(text) = self.reports.generating.get(&key).map(|g| g.text.clone()) else {
            return;
        };
        let Some(data) = self.week_data.as_mut() else {
            return;
        };
        if data.week.start != key.week_start {
            return;
        }
        let Some(slot) = data.panel.slots.iter_mut().find(|s| s.kind == key.kind) else {
            return;
        };
        slot.streaming = Some(text);
        self.week_view.load(data.clone());
    }

    /// Which reports the Week tab shows for `week`: the Summary, for the
    /// current and past weeks. (Recommended tasks moved to the Today tab.)
    fn panel_kinds(&self, week: WeekRef) -> Vec<ReportKind> {
        if week > self.current_week() {
            Vec::new()
        } else {
            vec![ReportKind::Summary]
        }
    }

    /// Every version of a report, newest first (cached until `invalidate`).
    pub(super) fn versions(&mut self, key: ReportKey) -> Vec<Report> {
        if let Some(v) = self.derived.versions.get(&key) {
            return v.clone();
        }
        match self.store.list_reports(key.kind, key.week_start) {
            Ok(v) => {
                self.derived.versions.insert(key, v.clone());
                v
            }
            Err(e) => {
                self.error = Some(e.to_string());
                Vec::new()
            }
        }
    }

    /// Whether `kind` for `week` has nothing to work from, and the digest
    /// of its facts now (cached until `invalidate`).
    fn facts(&mut self, kind: ReportKind, week: WeekRef) -> (bool, String) {
        let key = ReportKey {
            kind,
            week_start: week.start,
        };
        if let Some(f) = self.derived.facts.get(&key) {
            return f.clone();
        }
        let today = self.today();
        let empty = match kind {
            ReportKind::Summary => report::summary_is_empty(&report::summary_facts(
                &self.tasks,
                &self.events,
                week,
                today,
                &self.tz,
            )),
            ReportKind::Plan => report::plan_is_empty(&report::plan_facts(
                &self.tasks,
                &self.events,
                week,
                today,
                &self.settings,
            )),
        };
        let f = (empty, self.prepare(kind, week).digest_json);
        self.derived.facts.insert(key, f.clone());
        f
    }

    /// Whether `kind` for `week` has nothing to work from, and whether it
    /// needs (re)generating (`report_stale`), given its `versions`.
    fn freshness(&mut self, kind: ReportKind, week: WeekRef, versions: &[Report]) -> (bool, bool) {
        let (empty, digest) = self.facts(kind, week);
        let stale = report_stale(
            kind,
            versions,
            &digest,
            self.reports.prompt_changed.get(&kind).copied(),
            empty,
        );
        (empty, stale)
    }

    fn prepare(&self, kind: ReportKind, week: WeekRef) -> Prepared {
        let today = self.today();
        match kind {
            ReportKind::Summary => report::prepare_summary(
                &self.tasks,
                &self.events,
                week,
                today,
                &self.tz,
                &self.settings,
            ),
            ReportKind::Plan => {
                report::prepare_plan(&self.tasks, &self.events, week, today, &self.settings)
            }
        }
    }

    pub(super) fn panel_data(&mut self, week: WeekRef) -> ReportPanelData {
        let blocked = self.blocked();
        let mut slots = Vec::new();
        for kind in self.panel_kinds(week) {
            let key = ReportKey {
                kind,
                week_start: week.start,
            };
            let versions = self.versions(key);
            let (empty, stale) = self.freshness(kind, week, &versions);
            slots.push(ReportSlot {
                kind,
                versions,
                streaming: self.reports.generating.get(&key).map(|g| g.text.clone()),
                blocked: blocked.clone(),
                empty,
                stale,
            });
        }
        ReportPanelData { week, slots }
    }

    /// The schedule's due reports and what the shown tab needs.
    pub(crate) fn catch_up(&mut self) -> Task<Message> {
        Task::batch([self.run_due(), self.update_week_reports()])
    }

    /// Aborts what browsing started and is no longer shown: generations
    /// not started by the schedule for a week other than the current or
    /// the shown one, and later-day recommendations for another day.
    fn abort_browsing(&mut self) {
        let current = self.current_week().start;
        let shown = self.week_of(self.selected).start;
        let mut aborted = false;
        self.reports.generating.retain(|key, g| {
            let keep = keep_generation(g.scheduled, key.week_start, current, shown);
            if !keep {
                g.abort.abort();
                aborted = true;
            }
            keep
        });
        if self.later_days.abort_other_days(self.selected) {
            aborted = true;
        }
        if aborted {
            self.must_refresh = true;
        }
    }

    /// Generates the reports the shown tab needs that are out of date, once
    /// each (none while one is in flight, and failures back off): the Week
    /// tab's Summary, and the Today tab's recommended tasks (today's, or a
    /// later day's). First aborts what browsing left behind.
    pub(crate) fn update_week_reports(&mut self) -> Task<Message> {
        self.abort_browsing();
        if self.blocked().is_some() {
            return Task::none();
        }
        let (week, kinds) = match self.tab {
            Tab::Week => {
                let week = self.week_of(self.selected);
                (week, self.panel_kinds(week))
            }
            Tab::Today if self.selected == self.today() => {
                (self.current_week(), vec![ReportKind::Plan])
            }
            Tab::Today => return self.update_day_recs(),
            _ => return Task::none(),
        };
        let mut tasks = Vec::new();
        for kind in kinds {
            let key = ReportKey {
                kind,
                week_start: week.start,
            };
            if kind == ReportKind::Summary && self.reports.summary_waiting() {
                continue;
            }
            let versions = self.versions(key);
            let (empty, stale) = self.freshness(kind, week, &versions);
            if !empty && stale && !self.reports.backing_off(key) {
                tasks.push(self.generate(kind, week, false));
            }
        }
        Task::batch(tasks)
    }

    /// Forgets every failure, so the next catch-up tries at once (the
    /// endpoint or key changed).
    pub(super) fn reset_backoff(&mut self) {
        self.reports.failures.clear();
        self.reports.last_attempt.clear();
        self.later_days.clear_failures();
    }

    /// Starts streaming a report unless one is already in flight. A client
    /// that can't be built counts as a failed attempt (it backs off).
    fn generate(&mut self, kind: ReportKind, week: WeekRef, scheduled: bool) -> Task<Message> {
        let key = ReportKey {
            kind,
            week_start: week.start,
        };
        if self.reports.generating.contains_key(&key) {
            return Task::none();
        }
        let route = match self.plan_route(kind) {
            Ok(route) => route,
            Err(e) => {
                self.error = Some(e);
                self.reports.record_failure(key);
                return Task::none();
            }
        };
        let prepared = self.prepare(kind, week);
        let stream = route.client().chat_stream(prepared.request.clone());
        self.reports.next_generation += 1;
        let id = self.reports.next_generation;
        let (task, abort) = Task::run(stream, move |item| Message::Stream(key, id, item))
            .chain(Task::done(Message::StreamEnded(key, id)))
            .abortable();
        self.reports.generating.insert(
            key,
            Generation {
                prepared,
                text: String::new(),
                route,
                scheduled,
                abort,
                id,
            },
        );
        self.reports.last_attempt.insert(key, Timestamp::now());
        self.must_refresh = true;
        task
    }

    /// Stores the finished (or failed) report and forgets the generation.
    /// A failure is counted from the stored row (an unreadable plan is
    /// stored `Failed`). A scheduled summary that became ready sets the
    /// Week badge.
    fn finish(
        &mut self,
        key: ReportKey,
        id: u64,
        result: Result<ChatResponse, LlmError>,
    ) -> Task<Message> {
        if self.reports.generating.get(&key).is_none_or(|g| g.id != id) {
            return Task::none();
        }
        let Some(g) = self.reports.generating.remove(&key) else {
            return Task::none();
        };
        self.invalidate();
        let (requested_model, host) = g.route.answered_by();
        let stored = report::finish(&self.store, g.prepared, result, &requested_model, host);
        let ready = match &stored {
            Ok(r) => r.status == ReportStatus::Ready,
            Err(e) => {
                self.error = Some(e.to_string());
                false
            }
        };
        if ready {
            self.reports.failures.remove(&key);
        } else {
            // The wait before the next try counts from the failure, not
            // from when a long attempt began.
            self.reports.record_failure(key);
        }
        if stored.is_err() {
            return Task::none();
        }
        if ready && g.scheduled && key.kind == ReportKind::Summary && self.tab != Tab::Week {
            self.week_badge = true;
        }
        // Tasks may have changed while it ran: catch up straight away (a
        // failure is backing off now, so this doesn't retry it).
        if (key.kind == ReportKind::Summary && self.tab == Tab::Week)
            || (key.kind == ReportKind::Plan && self.decision_active())
        {
            return self.update_week_reports();
        }
        Task::none()
    }

    /// The schedule (PLAN.md section 8), at launch and every minute: from
    /// 16:00 on the week's last working day the summary is kept up to date
    /// in the background (generated when stale: out of date, or its newest
    /// attempt failed). Recommended tasks aren't scheduled: the Today tab
    /// makes them when it shows them. Waits quietly while locked or
    /// unconfigured; failures back off.
    fn run_due(&mut self) -> Task<Message> {
        if self.blocked().is_some() || self.tasks.is_empty() {
            return Task::none();
        }
        let jobs = due_reports(&self.now(), &self.settings.schedule, &[]);
        let mut tasks = Vec::new();
        for job in jobs {
            let key = ReportKey {
                kind: job.kind,
                week_start: job.week.start,
            };
            let versions = self.versions(key);
            let (empty, stale) = self.freshness(job.kind, job.week, &versions);
            if stale && !empty && !self.reports.backing_off(key) {
                tasks.push(self.generate(job.kind, job.week, true));
            }
        }
        Task::batch(tasks)
    }

    pub(crate) fn panel_action(&mut self, action: report_panel::Action) -> Task<Message> {
        match action {
            report_panel::Action::None => Task::none(),
            report_panel::Action::Generate(kind, week) => {
                self.reports.failures.remove(&ReportKey {
                    kind,
                    week_start: week.start,
                });
                self.generate(kind, week, false)
            }
            report_panel::Action::SaveEdit(id, text) => {
                if let Err(e) = self.store.set_report_edit(id, text.as_deref()) {
                    self.error = Some(e.to_string());
                }
                self.invalidate();
                Task::none()
            }
            report_panel::Action::Copy(text) => iced::clipboard::write(text),
        }
    }

    /// The Weekly Summary's wait after an edit may be over
    /// (`Message::SummaryWait`): if so, catches up what the tab shows.
    pub(crate) fn summary_wait_over(&mut self) -> Task<Message> {
        if self.reports.summary_waiting() {
            Task::none()
        } else {
            self.reports.summary_wait = None;
            self.update_week_reports()
        }
    }

    /// A report's stream ended (`Message::StreamEnded`).
    pub(crate) fn stream_ended(&mut self, key: ReportKey, id: u64) -> Task<Message> {
        // Only still generating if the stream ended without `Done`.
        self.finish(key, id, Err(LlmError::Truncated))
    }

    /// A piece of a streamed report (`Message::Stream`).
    pub(crate) fn stream(
        &mut self,
        key: ReportKey,
        id: u64,
        item: Result<ChatEvent, LlmError>,
    ) -> Task<Message> {
        match item {
            Ok(ChatEvent::Delta(d)) => {
                if let Some(g) = self.reports.generating.get_mut(&key).filter(|g| g.id == id) {
                    g.text.push_str(&d);
                    self.show_streaming(key);
                }
                Task::none()
            }
            Ok(ChatEvent::Done { model }) => {
                let text = self
                    .reports
                    .generating
                    .get(&key)
                    .map(|g| g.text.clone())
                    .unwrap_or_default();
                self.finish(key, id, Ok(ChatResponse { text, model }))
            }
            Err(e) => self.finish(key, id, Err(e)),
        }
    }
}

/// Whether a report needs (re)generating, from its `versions` (newest
/// first) and the `digest` of its facts now. Judged against the newest
/// `Ready` version: an edited one never needs it; otherwise it does when
/// the newest attempt failed, or the ready version was built from other
/// facts, an older plan prompt, or before the prompt was changed
/// (`prompt_changed`). With no versions, it does unless `empty`.
fn report_stale(
    kind: ReportKind,
    versions: &[Report],
    digest: &str,
    prompt_changed: Option<Timestamp>,
    empty: bool,
) -> bool {
    let ready = versions.iter().find(|r| r.status == ReportStatus::Ready);
    match ready {
        Some(r) if r.edited_md.is_some() => false,
        _ if versions
            .first()
            .is_some_and(|r| r.status == ReportStatus::Failed) =>
        {
            true
        }
        Some(r) => {
            // Recommendations made with an older prompt (the weekly list
            // before it became today's top three) are redone.
            (kind == ReportKind::Plan && r.prompt_version < report::PROMPT_VERSION)
                || r.digest_json != digest
                || prompt_changed.is_some_and(|at| r.created_at < at)
        }
        None => !empty,
    }
}

/// After `failures` failed attempts in a row, the last at `last_attempt`,
/// whether to wait before the next: `RETRY_AFTER_MINUTES`, doubling with
/// each further failure, up to `RETRY_MAX_MINUTES`.
pub(super) fn backoff_active(
    failures: u32,
    last_attempt: Option<Timestamp>,
    now: Timestamp,
) -> bool {
    if failures == 0 {
        return false;
    }
    let wait = RETRY_AFTER_MINUTES
        .saturating_mul(1 << (failures - 1).min(10))
        .min(RETRY_MAX_MINUTES);
    last_attempt.is_some_and(|t| now.duration_since(t) < SignedDuration::from_mins(wait))
}

/// Whether a generation stays when the user browses: one the schedule
/// started, or one for the current week or the shown one.
fn keep_generation(scheduled: bool, week_start: Date, current: Date, shown: Date) -> bool {
    scheduled || week_start == current || week_start == shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use fortnight_core::domain::NewReport;
    use fortnight_core::sqlite::SqliteStore;
    use jiff::civil::date;

    fn report(status: ReportStatus, digest: &str) -> Report {
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .insert_report(NewReport {
                kind: ReportKind::Summary,
                week_start: date(2026, 11, 16),
                status,
                digest_json: digest.into(),
                content_md: (status == ReportStatus::Ready).then(|| "text".into()),
                model: Some("m".into()),
                endpoint_host: None,
                prompt_version: report::PROMPT_VERSION,
                error: (status == ReportStatus::Failed).then(|| "down".into()),
            })
            .unwrap()
    }

    #[test]
    fn a_failed_newest_attempt_is_stale_unless_edited() {
        use ReportStatus::{Failed, Ready};
        let stale = |versions: &[Report], empty| {
            report_stale(ReportKind::Summary, versions, "d", None, empty)
        };
        // Nothing yet: wanted unless there is nothing to work from.
        assert!(stale(&[], false));
        assert!(!stale(&[], true));
        // Ready from the same facts: up to date; other facts: stale.
        assert!(!stale(&[report(Ready, "d")], false));
        assert!(stale(&[report(Ready, "old")], false));
        // A failure from the same facts doesn't count as up to date,
        // with or without an earlier ready version.
        assert!(stale(&[report(Failed, "d")], false));
        assert!(stale(&[report(Failed, "d"), report(Ready, "d")], false));
        // An older failure under a newer ready version is history.
        assert!(!stale(&[report(Ready, "d"), report(Failed, "d")], false));
        // An edited ready version still wins.
        let mut edited = report(Ready, "old");
        edited.edited_md = Some("mine".into());
        assert!(!stale(&[report(Failed, "d"), edited], false));
        // A prompt change after the ready version makes it stale.
        let r = report(Ready, "d");
        let later = r.created_at + SignedDuration::from_secs(1);
        assert!(report_stale(
            ReportKind::Summary,
            &[r],
            "d",
            Some(later),
            false
        ));
    }

    #[test]
    fn backoff_doubles_up_to_a_day() {
        let t0 = Timestamp::UNIX_EPOCH;
        let after = |mins| t0 + SignedDuration::from_mins(mins);
        assert!(!backoff_active(0, Some(t0), t0));
        assert!(backoff_active(1, Some(t0), after(9)));
        assert!(!backoff_active(1, Some(t0), after(10)));
        assert!(backoff_active(2, Some(t0), after(19)));
        assert!(!backoff_active(2, Some(t0), after(20)));
        assert!(backoff_active(30, Some(t0), after(24 * 60 - 1)));
        assert!(!backoff_active(30, Some(t0), after(24 * 60)));
        // No attempt on record: nothing to wait for.
        assert!(!backoff_active(3, None, t0));
    }

    #[test]
    fn browsing_keeps_only_scheduled_current_and_shown_weeks() {
        let (past, current, shown) = (date(2026, 11, 2), date(2026, 11, 16), date(2026, 11, 9));
        assert!(!keep_generation(false, past, current, shown));
        assert!(keep_generation(true, past, current, shown));
        assert!(keep_generation(false, current, current, shown));
        assert!(keep_generation(false, shown, current, shown));
    }
}
