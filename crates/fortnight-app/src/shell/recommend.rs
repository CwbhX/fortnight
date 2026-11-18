//! Recommended tasks on the Today tab: today's (the newest ready version of
//! the current week's Recommended tasks report) and a later day's (worked
//! out on demand, kept in memory only), and the route a recommendations
//! request takes (the decision model first, when one is set).

use crate::{Message, Ready};
use fortnight_app::top_bar::Tab;
use fortnight_app::views::day;
use fortnight_core::domain::{ReportKey, ReportKind, ReportStatus, Task as CoreTask};
use fortnight_core::fallback::{Endpoint, PlanRoute};
use fortnight_core::llm::{ChatResponse, LlmError};
use fortnight_core::report;
use iced::Task;
use jiff::Timestamp;
use jiff::civil::Date;
use std::collections::HashMap;

use super::reports::backoff_active;

/// Recommended tasks for later days.
#[derive(Default)]
pub(crate) struct LaterDays {
    /// Recommended tasks for later days (not stored as reports): each with
    /// the facts it was worked out from, redone when those change.
    recs: HashMap<Date, (String, Vec<day::Recommended>)>,
    /// Later days whose recommendations are being worked out: from which
    /// facts, and the request (aborted on Lock or when the day is left).
    pending: HashMap<Date, (String, iced::task::Handle)>,
    /// Failures in a row per later day and when the last one was, for the
    /// same retry backoff as reports.
    failures: HashMap<Date, (u32, Timestamp)>,
}

impl LaterDays {
    /// The date moved on to `today`: forgets the days now past, aborting
    /// their requests.
    pub(crate) fn forget_past(&mut self, today: Date) {
        self.recs.retain(|day, _| *day > today);
        self.failures.retain(|day, _| *day > today);
        self.pending.retain(|day, (_, handle)| {
            let keep = *day > today;
            if !keep {
                handle.abort();
            }
            keep
        });
    }

    /// What they are worked out with changed (the prompt, a model):
    /// forgets every later day's list and failures, and aborts the
    /// requests on their way (a reply to the old prompt is dropped).
    pub(crate) fn forget_all(&mut self) {
        self.recs.clear();
        self.failures.clear();
        for (_, (_, handle)) in self.pending.drain() {
            handle.abort();
        }
    }

    /// Aborts the requests for days other than `selected`; whether any was.
    pub(super) fn abort_other_days(&mut self, selected: Date) -> bool {
        let mut aborted = false;
        self.pending.retain(|day, (_, handle)| {
            let keep = *day == selected;
            if !keep {
                handle.abort();
                aborted = true;
            }
            keep
        });
        aborted
    }

    /// Aborts every request (Lock).
    pub(super) fn abort_all(&mut self) {
        for (_, (_, handle)) in self.pending.drain() {
            handle.abort();
        }
    }

    /// Forgets every failure (the endpoint or key changed).
    pub(super) fn clear_failures(&mut self) {
        self.failures.clear();
    }
}

impl Ready {
    /// The recommended tasks the Today tab shows for the selected day, and
    /// whether they are being worked out.
    pub(super) fn day_recommendations(&mut self, today: Date) -> (Vec<day::Recommended>, bool) {
        let recommended = if self.selected == today {
            self.recommended_today()
        } else {
            self.later_days
                .recs
                .get(&self.selected)
                .map(|(_, list)| list.clone())
                .unwrap_or_default()
        };
        let recommending = if self.selected == today {
            self.reports.is_generating(&ReportKey {
                kind: ReportKind::Plan,
                week_start: self.current_week().start,
            })
        } else {
            self.later_days.pending.contains_key(&self.selected)
        };
        (recommended, recommending)
    }

    /// Today's recommended tasks: the newest ready version of the current
    /// week's Recommended tasks report that still names an open task on
    /// the Today tab's list or overdue (as the tab shows them: a chat model
    /// may not keep to that rule). An overdue task stays eligible until it
    /// is done or deleted. Falling back to an older version keeps the last
    /// good list on screen when the newest names none of them, e.g. while
    /// a new one is worked out.
    fn recommended_today(&mut self) -> Vec<day::Recommended> {
        if let Some(list) = &self.derived.recommended_today {
            return list.clone();
        }
        let today = self.today();
        let versions = self.versions(ReportKey {
            kind: ReportKind::Plan,
            week_start: self.current_week().start,
        });
        let list = versions
            .iter()
            .filter(|r| r.status == ReportStatus::Ready)
            .filter_map(|r| r.text().and_then(report::parse_recommendations))
            .map(|rec| {
                recommended(rec)
                    .into_iter()
                    .filter(|r| {
                        self.task(r.id)
                            .is_some_and(|t| t.on_today_list(today) || t.is_overdue(today))
                    })
                    .collect::<Vec<_>>()
            })
            .find(|list| !list.is_empty())
            .unwrap_or_default();
        self.derived.recommended_today = Some(list.clone());
        list
    }

    /// Works out the recommended tasks of the later day the Today tab
    /// shows, unless they are known for its current facts or on their way.
    /// Only the open tasks planned for that day take part: overdue ones are
    /// today's business. Not stored; a failure keeps what was shown and is
    /// retried with the reports' backoff.
    pub(super) fn update_day_recs(&mut self) -> Task<Message> {
        let day = self.selected;
        if self.tab != Tab::Today || day <= self.today() || self.blocked().is_some() {
            return Task::none();
        }
        let tasks: Vec<CoreTask> = self
            .tasks
            .iter()
            .filter(|t| t.status.is_open() && t.planned_for == Some(day))
            .cloned()
            .collect();
        if tasks.is_empty() {
            self.later_days.recs.remove(&day);
            return Task::none();
        }
        let prepared =
            report::prepare_plan(&tasks, &self.events, self.week_of(day), day, &self.settings);
        let facts = prepared.digest_json;
        if self
            .later_days
            .recs
            .get(&day)
            .is_some_and(|(f, _)| *f == facts)
            || self
                .later_days
                .pending
                .get(&day)
                .is_some_and(|(f, _)| *f == facts)
        {
            return Task::none();
        }
        let (failures, last) = self.later_days.failures.get(&day).copied().unzip();
        if backoff_active(failures.unwrap_or(0), last, Timestamp::now()) {
            return Task::none();
        }
        let Ok(route) = self.plan_route(ReportKind::Plan) else {
            return Task::none();
        };
        let sent = facts.clone();
        let (task, handle) = Task::perform(route.client().chat(prepared.request), move |result| {
            Message::DayRecs(day, sent, result)
        })
        .abortable();
        if let Some((_, old)) = self.later_days.pending.insert(day, (facts, handle)) {
            old.abort();
        }
        self.must_refresh = true;
        task
    }

    /// Where a report of `kind` goes: the main endpoint, and for
    /// recommended tasks the decision model first when one is set (and can
    /// be built).
    pub(super) fn plan_route(&self, kind: ReportKind) -> Result<PlanRoute, String> {
        let (client, host) = self.keys.client_for(&self.settings.llm)?;
        let decision = &self.settings.decision_llm;
        let decision = (kind == ReportKind::Plan && decision.is_configured())
            .then(|| {
                self.keys
                    .decision_client_for(decision, &self.settings.llm)
                    .ok()
            })
            .flatten()
            .map(|(client, host)| Endpoint { client, host });
        Ok(PlanRoute::new(Endpoint { client, host }, decision))
    }

    /// A later day's recommended tasks came back (`Message::DayRecs`).
    /// `None` when it is not the latest request for that day (the message
    /// ends there).
    pub(crate) fn day_recs_done(
        &mut self,
        day: Date,
        facts: String,
        result: Result<ChatResponse, LlmError>,
    ) -> Option<Task<Message>> {
        // Only the latest request for that day counts (one left
        // behind or aborted doesn't).
        if self
            .later_days
            .pending
            .get(&day)
            .is_none_or(|(f, _)| *f != facts)
        {
            return None;
        }
        self.later_days.pending.remove(&day);
        match result
            .ok()
            .and_then(|r| report::parse_recommendations(&r.text))
        {
            Some(rec) => {
                self.later_days.recs.insert(day, (facts, recommended(rec)));
                self.later_days.failures.remove(&day);
            }
            // Keeps what was shown; retried after the backoff.
            None => {
                let failures = self.later_days.failures.get(&day).map_or(0, |f| f.0);
                self.later_days
                    .failures
                    .insert(day, (failures + 1, Timestamp::now()));
            }
        }
        // Tasks may have changed while it ran.
        Some(self.update_day_recs())
    }
}

/// The tasks a recommendation names, in its order.
fn recommended(rec: report::Recommendation) -> Vec<day::Recommended> {
    rec.tasks
        .into_iter()
        .map(|t| day::Recommended { id: t.id })
        .collect()
}
