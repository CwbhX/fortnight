//! Summary facts (the week behind), their Markdown, and the summary
//! prompt.

use super::format::*;
use super::history::*;
use super::{DATA_RULE, Prepared, facts_json};
use crate::domain::{Event, EventKind, ReportKind, Settings, Task, TaskId, TaskStatus, WeekRef};
use crate::llm::ChatRequest;
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Facts for the weekly summary of `week` (the week behind you).
/// Private tasks are left out of every list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SummaryFacts {
    pub week_start: Date,
    pub week_end: Date,
    /// The day the facts describe: today, or `week_end` for a past week.
    /// Overdue counts are relative to it.
    pub as_of: Date,
    /// Done tasks whose completion falls in the week, oldest first, with
    /// their log lines from the week.
    pub completed: Vec<CompletedTask>,
    /// Tasks worked on during the week (a log line, a status change to
    /// doing, or an edit) that were still open at the week's end, including
    /// ones completed later. With their log lines from the week.
    pub progressed: Vec<ProgressedTask>,
    /// Open tasks touched during the week without recorded activity (edited,
    /// or status `Doing`) that are not already in `progressed`, `missed` or
    /// `next_week`. Mostly data from before history was recorded.
    pub in_progress: Vec<OpenTask>,
    /// Open tasks due on or before `as_of`: slipped, overdue or due today.
    pub missed: Vec<MissedTask>,
    /// Deadlines moved later during the week.
    pub slipped: Vec<SlippedTask>,
    /// Open tasks due after `as_of`, through the end of the following week.
    pub next_week: Vec<DueTask>,
    /// Tasks created during the week, in any status.
    pub added: Vec<AddedTask>,
    /// Free-standing log lines written during the week: the Today tab's
    /// day notes (side work, meetings, blockers, anything not a task).
    /// Oldest first.
    pub log_lines: Vec<LogEntry>,
    /// Completed counts per project; `project: null` is the no-project
    /// bucket. Named projects first, alphabetically.
    pub completed_by_project: Vec<ProjectCount>,
}

/// One log line, on its local date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogEntry {
    pub date: Date,
    pub weekday: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompletedTask {
    pub id: i64,
    pub title: String,
    pub project: Option<String>,
    pub completed_on: Date,
    pub completed_weekday: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub log_lines: Vec<LogEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProgressedTask {
    pub id: i64,
    pub title: String,
    pub project: Option<String>,
    /// Status at the end of the week: `todo` or `doing`.
    pub status: TaskStatus,
    /// The status changed to doing during the week.
    pub started_this_week: bool,
    /// The deadline at the end of the week.
    pub due_on: Option<Date>,
    pub due_weekday: Option<&'static str>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub log_lines: Vec<LogEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpenTask {
    pub id: i64,
    pub title: String,
    pub project: Option<String>,
    pub due_on: Option<Date>,
    pub due_weekday: Option<&'static str>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MissedTask {
    pub id: i64,
    pub title: String,
    pub project: Option<String>,
    pub due_on: Date,
    pub due_weekday: &'static str,
    /// Days between the due date and `as_of`; 0 = due on `as_of`.
    pub days_overdue: i32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

/// A deadline moved later during the week, possibly several times.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SlippedTask {
    pub id: i64,
    pub title: String,
    pub project: Option<String>,
    /// The deadline before the week's first later move.
    pub from: Date,
    pub from_weekday: &'static str,
    /// The deadline after the week's last later move.
    pub to: Date,
    pub to_weekday: &'static str,
    /// Days from `from` to `to`.
    pub days_later: i32,
    /// How many times the deadline moved later this week.
    pub times: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DueTask {
    pub id: i64,
    pub title: String,
    pub project: Option<String>,
    pub due_on: Date,
    pub due_weekday: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AddedTask {
    pub id: i64,
    pub title: String,
    pub project: Option<String>,
    pub created_on: Date,
    /// Status at the end of the week.
    pub status: TaskStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectCount {
    pub project: Option<String>,
    pub completed: usize,
}

/// Builds summary facts. `events` is the whole history (any order); only
/// events whose local date (via `tz`) falls in `week` count. `tz` also
/// converts `completed_at` and `created_at` to local dates. `today` is
/// clamped to the week's end, so a past week is described as of its last
/// day.
pub fn summary_facts(
    tasks: &[Task],
    events: &[Event],
    week: WeekRef,
    today: Date,
    tz: &TimeZone,
) -> SummaryFacts {
    let lw = LocalWeek { week, tz };
    let week_end = week.end();
    let as_of = today.min(week_end);
    let next = week.next();
    let tasks = shareable(tasks);
    let history = History::new(&tasks, events);

    // Judged as of the end of the week, so a catch-up run or a later
    // regeneration describes the week as it was, not as things are now.
    let mut done: Vec<(&Task, Timestamp, Date)> = tasks
        .iter()
        .copied()
        .filter_map(|t| {
            lw.completed_in_week(t, history.of(t.id))
                .map(|ts| (t, ts, lw.date(ts)))
        })
        .collect();
    done.sort_by_key(|(t, ts, _)| (*ts, t.id));
    let completed: Vec<CompletedTask> = done
        .iter()
        .map(|(t, _, d)| CompletedTask {
            id: t.id,
            title: clean_title(&t.title),
            project: t.project.clone(),
            completed_on: *d,
            completed_weekday: weekday_name(d.weekday()),
            notes: trim_notes(&t.notes),
            log_lines: lw.logs(history.of(t.id)),
        })
        .collect();

    let mut ordered = tasks.clone();
    sort_by_due(&mut ordered);

    let progressed: Vec<ProgressedTask> = ordered
        .iter()
        .filter_map(|t| {
            let all = history.of(t.id);
            let status = lw.status_at_end(t, all);
            if !status.is_open() {
                return None;
            }
            let in_week = lw.events(all);
            let started = in_week.iter().any(|e| {
                matches!(
                    e.kind,
                    EventKind::StatusChanged {
                        to: TaskStatus::Doing,
                        ..
                    }
                )
            });
            let worked = started
                || in_week
                    .iter()
                    .any(|e| matches!(e.kind, EventKind::Log { .. } | EventKind::Edited { .. }));
            let due_on = lw.due_at_end(t, all);
            worked.then(|| ProgressedTask {
                id: t.id,
                title: clean_title(&t.title),
                project: t.project.clone(),
                status,
                started_this_week: started,
                due_on,
                due_weekday: due_on.map(|d| weekday_name(d.weekday())),
                notes: trim_notes(&t.notes),
                log_lines: lw.logs(all),
            })
        })
        .collect();
    let progressed_ids: BTreeSet<TaskId> = progressed.iter().map(|p| p.id).collect();

    // Open at the end of the week, with the deadline it had then.
    let open: Vec<(&Task, Option<Date>, TaskStatus)> = ordered
        .iter()
        .copied()
        .filter(|t| lw.existed(t))
        .map(|t| {
            let all = history.of(t.id);
            (t, lw.due_at_end(t, all), lw.status_at_end(t, all))
        })
        .filter(|(_, _, status)| status.is_open())
        .collect();

    let missed: Vec<MissedTask> = open
        .iter()
        .filter_map(|(t, due, _)| due.filter(|d| *d <= as_of).map(|d| (t, d)))
        .map(|(t, d)| MissedTask {
            id: t.id,
            title: clean_title(&t.title),
            project: t.project.clone(),
            due_on: d,
            due_weekday: weekday_name(d.weekday()),
            days_overdue: days_between(d, as_of),
            notes: trim_notes(&t.notes),
        })
        .collect();

    let next_week: Vec<DueTask> = open
        .iter()
        .filter_map(|(t, due, _)| {
            due.filter(|d| *d > as_of && *d <= next.end())
                .map(|d| (t, d))
        })
        .map(|(t, d)| DueTask {
            id: t.id,
            title: clean_title(&t.title),
            project: t.project.clone(),
            due_on: d,
            due_weekday: weekday_name(d.weekday()),
        })
        .collect();

    let in_progress: Vec<OpenTask> = open
        .iter()
        .filter(|(t, _, _)| !progressed_ids.contains(&t.id))
        .filter(|(_, due, _)| !due.is_some_and(|d| d <= next.end()))
        .filter(|(t, _, status)| {
            *status == TaskStatus::Doing
                || (week.contains(lw.date(t.updated_at)) && lw.date(t.created_at) < week.start)
        })
        .map(|(t, due, _)| OpenTask {
            id: t.id,
            title: clean_title(&t.title),
            project: t.project.clone(),
            due_on: *due,
            due_weekday: due.map(|d| weekday_name(d.weekday())),
            notes: trim_notes(&t.notes),
        })
        .collect();

    let mut slipped: Vec<(Timestamp, SlippedTask)> = Vec::new();
    for t in &tasks {
        let in_week = lw.events(history.of(t.id));
        // Net change over the week: a deadline moved later and back again
        // did not slip.
        let changes: Vec<(Timestamp, Option<Date>, Option<Date>)> = in_week
            .iter()
            .filter_map(|e| match e.kind {
                EventKind::DueChanged { from, to } => Some((e.at, from, to)),
                _ => None,
            })
            .collect();
        let moves: Vec<_> = in_week
            .iter()
            .filter(|e| later_move(&e.kind).is_some())
            .collect();
        let net = match (changes.first(), changes.last()) {
            (Some(&(at, Some(from), _)), Some(&(_, _, Some(to)))) if to > from => {
                Some((at, from, to))
            }
            _ => None,
        };
        if let Some((at, from, to)) = net {
            slipped.push((
                at,
                SlippedTask {
                    id: t.id,
                    title: clean_title(&t.title),
                    project: t.project.clone(),
                    from,
                    from_weekday: weekday_name(from.weekday()),
                    to,
                    to_weekday: weekday_name(to.weekday()),
                    days_later: days_between(from, to),
                    times: moves.len(),
                },
            ));
        }
    }
    slipped.sort_by_key(|(at, s)| (*at, s.id));

    let mut created: Vec<(&Task, Date)> = tasks
        .iter()
        .map(|t| (*t, lw.date(t.created_at)))
        .filter(|(_, d)| week.contains(*d))
        .collect();
    created.sort_by_key(|(t, _)| (t.created_at, t.id));
    let added = created
        .iter()
        .map(|(t, d)| AddedTask {
            id: t.id,
            title: clean_title(&t.title),
            project: t.project.clone(),
            created_on: *d,
            status: lw.status_at_end(t, history.of(t.id)),
        })
        .collect();

    let log_lines = lw.logs(&history.free_logs);

    // `Option<String>` sorts `None` first; flip so named projects lead.
    let mut counts: BTreeMap<(bool, Option<String>), usize> = BTreeMap::new();
    for c in &completed {
        *counts
            .entry((c.project.is_none(), c.project.clone()))
            .or_default() += 1;
    }
    let completed_by_project = counts
        .into_iter()
        .map(|((_, project), completed)| ProjectCount { project, completed })
        .collect();

    SummaryFacts {
        week_start: week.start,
        week_end,
        as_of,
        completed,
        progressed,
        in_progress,
        missed,
        slipped: slipped.into_iter().map(|(_, s)| s).collect(),
        next_week,
        added,
        log_lines,
        completed_by_project,
    }
}

/// Facts as Markdown, for the CLI's `--facts-only`.
pub fn summary_facts_markdown(facts: &SummaryFacts) -> String {
    let mut md = format!(
        "**Week:** {} to {} {}\n",
        short_date(facts.week_start),
        short_date(facts.week_end),
        facts.week_end.year()
    );

    section(
        &mut md,
        "Completed",
        facts.completed.len(),
        "Nothing completed this week.",
    );
    for t in &facts.completed {
        let _ = writeln!(
            md,
            "- {}{} · {}",
            t.title,
            project_suffix(&t.project),
            short_date(t.completed_on)
        );
        log_bullets(&mut md, &t.log_lines);
    }

    section(
        &mut md,
        "In progress",
        facts.progressed.len() + facts.in_progress.len(),
        "Nothing in progress.",
    );
    for t in &facts.progressed {
        let started = if t.started_this_week {
            " · started"
        } else {
            ""
        };
        let _ = writeln!(
            md,
            "- {}{}{}{started}",
            t.title,
            project_suffix(&t.project),
            due_suffix(t.due_on)
        );
        log_bullets(&mut md, &t.log_lines);
    }
    for t in &facts.in_progress {
        let _ = writeln!(
            md,
            "- {}{}{}",
            t.title,
            project_suffix(&t.project),
            due_suffix(t.due_on)
        );
    }

    section(
        &mut md,
        "Overdue or due now",
        facts.missed.len(),
        "Nothing overdue.",
    );
    for t in &facts.missed {
        let late = match t.days_overdue {
            0 => String::new(),
            1 => " (1 day overdue)".to_string(),
            n => format!(" ({n} days overdue)"),
        };
        let _ = writeln!(
            md,
            "- {}{} · due {}{late}",
            t.title,
            project_suffix(&t.project),
            short_date(t.due_on)
        );
    }

    section(
        &mut md,
        "Deadlines moved",
        facts.slipped.len(),
        "No deadlines moved later.",
    );
    for t in &facts.slipped {
        let times = if t.times > 1 {
            format!(" ({} times)", t.times)
        } else {
            String::new()
        };
        let _ = writeln!(
            md,
            "- {}{} · {} to {}{times}",
            t.title,
            project_suffix(&t.project),
            short_date(t.from),
            short_date(t.to)
        );
    }

    section(
        &mut md,
        "Coming up",
        facts.next_week.len(),
        "Nothing due before the end of next week.",
    );
    for t in &facts.next_week {
        let _ = writeln!(
            md,
            "- {}{} · due {}",
            t.title,
            project_suffix(&t.project),
            short_date(t.due_on)
        );
    }

    section(
        &mut md,
        "Added",
        facts.added.len(),
        "No tasks added this week.",
    );
    for t in &facts.added {
        let done = if t.status == TaskStatus::Done {
            " (done)"
        } else {
            ""
        };
        let _ = writeln!(
            md,
            "- {}{} · {}{done}",
            t.title,
            project_suffix(&t.project),
            short_date(t.created_on)
        );
    }

    section(&mut md, "Log", facts.log_lines.len(), "No other log lines.");
    for l in &facts.log_lines {
        let _ = writeln!(md, "- {}: {}", short_date(l.date), l.text);
    }

    if !facts.completed_by_project.is_empty() {
        md.push_str("\n### Completed by project\n\n");
        for c in &facts.completed_by_project {
            let name = c.project.as_deref().unwrap_or("No project");
            let _ = writeln!(md, "- {name}: {}", c.completed);
        }
    }
    md
}

/// True when the week has nothing to summarise (no completed, progressed,
/// missed, slipped or added tasks and no log lines). Tasks only in progress
/// don't count.
pub fn summary_is_empty(facts: &SummaryFacts) -> bool {
    facts.completed.is_empty()
        && facts.progressed.is_empty()
        && facts.missed.is_empty()
        && facts.slipped.is_empty()
        && facts.log_lines.is_empty()
        && facts.added.is_empty()
}

/// The built-in system prompt for the work summary. The user can replace it
/// in Settings (`Settings::summary_prompt`); `DATA_RULE` is always added.
pub const DEFAULT_SUMMARY_PROMPT: &str = "\
You write the weekly status update that the user sends to their manager.

Rules:
- Use only the facts provided. Do not invent work, metrics, outcomes, reasons or dates.
- Write in the first person (\"I shipped...\") for a manager who knows the projects but not the details.
- Lead with outcomes, not activity. Group items by project; tasks with no project and free-standing log lines go under \"Other\".
- Do no arithmetic. Dates, weekdays and day counts are already computed in the facts; use them as given.
- `log_lines` are the user's own notes from each day: side work, meetings, decisions, blockers, ideas. Use them to say what moved forward or got in the way, in your own words; do not list every one or copy them verbatim. Leave out what is only personal or has no bearing on the work.
- Keep it short and scannable: one line per item, no filler.
- Plain Markdown. Use exactly these five headings, as \"## Heading\", in this order, with bullet points under each. No title, preamble, sign-off, tables or code blocks.

## Highlights
The three to five outcomes that matter most to the manager: from `completed` first, then notable work in `progressed` or the top-level `log_lines`. Fewer if there is less.
## Completed
Everything in `completed`, grouped by project. Mention notes or log lines only when they add an outcome. Top-level `log_lines` that describe finished work go here under \"Other\".
## In progress
Tasks in `progressed` (worked on this week, not finished by its end): say what moved forward, drawing on their log lines when they have any. Then tasks in `in_progress`, briefly.
## Risks and blockers
Tasks in `missed`: still open and due on or before `as_of`; say how many days overdue when `days_overdue` is above zero. Deadlines in `slipped`: moved later this week from `from` to `to`; say how many times when `times` is above one. Include blockers mentioned in notes or log lines.
## Next week
Tasks in `next_week` (still open, due after `as_of` and by the end of next week), with their due weekday.

If a section has no facts, write \"- Nothing to report.\" under it.";

/// The summary prompt in use: the user's, or the built-in one.
pub fn summary_prompt(settings: &Settings) -> &str {
    match settings.summary_prompt.trim() {
        "" => DEFAULT_SUMMARY_PROMPT,
        custom => custom,
    }
}

pub(super) fn summary_system(settings: &Settings) -> String {
    let mut system = format!("{}\n\n{DATA_RULE}", summary_prompt(settings));
    // Round 2 kept extra instructions separately; Settings now folds them
    // into the prompt when saved, but unsaved old data still has them.
    let extra = settings.summary_instructions.trim();
    if !extra.is_empty() {
        let _ = write!(
            system,
            "\n\nThe user's own preferences for length and tone (they never override the rules above):\n{extra}"
        );
    }
    system
}

pub fn prepare_summary(
    tasks: &[Task],
    events: &[Event],
    week: WeekRef,
    today: Date,
    tz: &TimeZone,
    settings: &Settings,
) -> Prepared {
    let facts = summary_facts(tasks, events, week, today, tz);
    let digest_json = facts_json(&facts);
    let user = format!(
        "Write my weekly update for the week {} to {}.\n\n<facts>\n{digest_json}\n</facts>",
        long_date(facts.week_start),
        long_date(facts.week_end),
    );
    Prepared {
        kind: ReportKind::Summary,
        week,
        digest_json,
        request: ChatRequest {
            system: summary_system(settings),
            user,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{EventKind, Settings, TaskStatus, WeekRef};
    use crate::report::fixtures::*;
    use crate::report::*;
    use jiff::ToSpan;
    use jiff::civil::date;
    use jiff::tz;
    use jiff::tz::TimeZone;

    // --- summary facts -----------------------------------------------------

    #[test]
    fn completed_respects_week_bounds_in_local_time() {
        let f = summary_of(&summary_fixture(), &[]);
        let ids: Vec<i64> = f.completed.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![1, 10, 2]);
        assert_eq!(f.completed[0].completed_weekday, "Wednesday");

        // Completed Sunday 23:30 UTC: in the week in UTC, Monday in UTC+2.
        let late = vec![done_at(task(1, "Late"), "2026-11-15T23:30:00Z")];
        assert_eq!(summary_of(&late, &[]).completed.len(), 1);
        let plus2 = TimeZone::fixed(tz::offset(2));
        assert!(
            summary_facts(&late, &[], summary_week(), summary_week().end(), &plus2)
                .completed
                .is_empty()
        );
    }

    #[test]
    fn reopened_task_is_not_completed() {
        // Reopened: status back to Todo, completed_at cleared.
        let tasks = vec![updated(task(1, "Reopened"), "2026-11-11T10:00:00Z")];
        let f = summary_of(&tasks, &[]);
        assert!(f.completed.is_empty());
        assert!(f.completed_by_project.is_empty());
        assert_eq!(f.in_progress.len(), 1);

        // A done task without completed_at (bad data) is skipped, not a panic.
        let mut odd = task(2, "Odd");
        odd.status = TaskStatus::Done;
        assert!(summary_of(&[odd], &[]).completed.is_empty());
    }

    #[test]
    fn missed_next_week_and_counts() {
        let f = summary_of(&summary_fixture(), &[]);
        let missed: Vec<(i64, i32)> = f.missed.iter().map(|m| (m.id, m.days_overdue)).collect();
        assert_eq!(missed, vec![(6, 3), (7, 0)]);
        let next: Vec<i64> = f.next_week.iter().map(|t| t.id).collect();
        assert_eq!(next, vec![5, 11]);
        // Touched this week, created earlier, not due soon.
        let progress: Vec<i64> = f.in_progress.iter().map(|t| t.id).collect();
        assert_eq!(progress, vec![9]);
        let added: Vec<i64> = f.added.iter().map(|t| t.id).collect();
        assert_eq!(added, vec![8, 10]);
        let counts: Vec<(Option<&str>, usize)> = f
            .completed_by_project
            .iter()
            .map(|c| (c.project.as_deref(), c.completed))
            .collect();
        assert_eq!(
            counts,
            vec![(Some("Hiring"), 1), (Some("Website"), 1), (None, 1)]
        );
    }

    #[test]
    fn mid_week_counts_overdue_from_today() {
        // Wednesday: Thursday's and Sunday's tasks are not overdue yet.
        let wed = summary_week().start.saturating_add(2.days());
        let f = summary_facts(&summary_fixture(), &[], summary_week(), wed, &TimeZone::UTC);
        assert_eq!(f.as_of, wed);
        assert!(f.missed.is_empty());
        let next: Vec<i64> = f.next_week.iter().map(|t| t.id).collect();
        assert_eq!(next, vec![6, 7, 5, 11]);
        // A later "today" is clamped to the week's end.
        let later = summary_week().end().saturating_add(30.days());
        let f = summary_facts(
            &summary_fixture(),
            &[],
            summary_week(),
            later,
            &TimeZone::UTC,
        );
        assert_eq!(f.as_of, summary_week().end());
    }

    #[test]
    fn long_notes_are_trimmed() {
        let mut t = task(1, "Notes");
        t.notes = format!("  {}  ", "é".repeat(500));
        let t = done_at(t, "2026-11-10T10:00:00Z");
        let f = summary_of(&[t], &[]);
        let notes = &f.completed[0].notes;
        assert_eq!(notes.chars().count(), NOTES_MAX_CHARS + 1);
        assert!(notes.ends_with('…'));
    }

    #[test]
    fn progressed_needs_activity_in_the_week_and_open_at_its_end() {
        let f = summary_of(&summary_fixture(), &summary_events());
        let ids: Vec<(i64, TaskStatus, bool)> = f
            .progressed
            .iter()
            .map(|p| (p.id, p.status, p.started_this_week))
            .collect();
        // 5: started and logged; 9: logged and edited; 14: logged, done later.
        // Not 1 (completed in the week), 13 (dropped), 15 (private), 6 (only
        // deadline moves).
        assert_eq!(
            ids,
            vec![
                (5, TaskStatus::Doing, true),
                (9, TaskStatus::Todo, false),
                (14, TaskStatus::Todo, false),
            ]
        );
        let texts = |p: &ProgressedTask| {
            p.log_lines
                .iter()
                .map(|l| l.text.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            texts(&f.progressed[0]),
            vec!["Found the double-counting bug"]
        );
        // The log line from before the week is left out.
        assert_eq!(texts(&f.progressed[1]), vec!["Drafted the scoring scale"]);
        assert_eq!(f.progressed[2].log_lines[0].weekday, "Friday");
        // Progressed tasks are not repeated in `in_progress`.
        assert!(f.in_progress.iter().all(|t| t.id != 9));

        // Only a planned-day change or a status change to todo: not progress.
        let tasks = vec![task(1, "Quiet")];
        let events = vec![
            ev(
                1,
                Some(1),
                "2026-11-10T09:00:00Z",
                EventKind::PlannedChanged {
                    from: None,
                    to: Some(date(2026, 11, 12)),
                },
            ),
            status(
                2,
                1,
                "2026-11-11T09:00:00Z",
                TaskStatus::Done,
                TaskStatus::Todo,
            ),
        ];
        assert!(summary_of(&tasks, &events).progressed.is_empty());
    }

    #[test]
    fn progressed_uses_status_at_the_end_of_the_week() {
        let t = task(1, "Stopped later");
        let events = vec![
            log(1, Some(1), "2026-11-10T09:00:00Z", "Started"),
            status(
                2,
                1,
                "2026-11-18T09:00:00Z",
                TaskStatus::Doing,
                TaskStatus::Todo,
            ),
        ];
        let f = summary_of(&[t], &events);
        assert_eq!(f.progressed.len(), 1);
        assert_eq!(f.progressed[0].status, TaskStatus::Doing);

        // Done in the week but the task has no completed_at (bad data):
        // not progressed either.
        let events = vec![
            log(1, Some(1), "2026-11-10T09:00:00Z", "Started"),
            status(
                2,
                1,
                "2026-11-11T09:00:00Z",
                TaskStatus::Todo,
                TaskStatus::Done,
            ),
        ];
        let mut d = task(1, "Done");
        d.status = TaskStatus::Done;
        assert!(summary_of(&[d], &events).progressed.is_empty());
    }

    #[test]
    fn completed_tasks_gain_their_week_log_lines() {
        let tasks = vec![done_at(task(1, "Ship"), "2026-11-12T10:00:00Z")];
        let events = vec![
            log(1, Some(1), "2026-11-02T10:00:00Z", "Old"),
            log(2, Some(1), "2026-11-10T10:00:00Z", "  Wrote   the tests  "),
            log(3, Some(1), "2026-11-12T09:00:00Z", "Merged"),
        ];
        let f = summary_of(&tasks, &events);
        let logs = &f.completed[0].log_lines;
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].text, "Wrote the tests");
        assert_eq!(logs[0].date, date(2026, 11, 10));
        assert_eq!(logs[0].weekday, "Tuesday");
        assert_eq!(logs[1].text, "Merged");
        // Task log lines are not free-standing.
        assert!(f.log_lines.is_empty());
    }

    #[test]
    fn slipped_counts_later_moves_in_the_week() {
        let f = summary_of(&summary_fixture(), &summary_events());
        assert_eq!(f.slipped.len(), 1);
        let s = &f.slipped[0];
        assert_eq!(s.id, 6);
        assert_eq!((s.from, s.to), (date(2026, 11, 10), date(2026, 11, 12)));
        assert_eq!((s.from_weekday, s.to_weekday), ("Tuesday", "Thursday"));
        assert_eq!(s.days_later, 2);
        assert_eq!(s.times, 2);

        // Moved earlier, set, or cleared: not slipped.
        let tasks = vec![task(1, "A")];
        let events = vec![
            due_moved(
                1,
                1,
                "2026-11-10T09:00:00Z",
                Some(date(2026, 11, 20)),
                Some(date(2026, 11, 13)),
            ),
            due_moved(2, 1, "2026-11-11T09:00:00Z", None, Some(date(2026, 11, 20))),
            due_moved(3, 1, "2026-11-12T09:00:00Z", Some(date(2026, 11, 20)), None),
        ];
        assert!(summary_of(&tasks, &events).slipped.is_empty());
    }

    #[test]
    fn free_standing_log_lines_in_the_week() {
        let f = summary_of(&summary_fixture(), &summary_events());
        let logs: Vec<(Date, &str)> = f
            .log_lines
            .iter()
            .map(|l| (l.date, l.text.as_str()))
            .collect();
        assert_eq!(
            logs,
            vec![
                (date(2026, 11, 10), "Helped Sam debug the deploy"),
                (date(2026, 11, 14), "Ran the team retro"),
            ]
        );
    }

    #[test]
    fn catch_up_reports_the_week_as_it_was() {
        // Due Thu 12 Nov, still open at the end of that week, finished on
        // Mon 16 Nov before the catch-up summary for the 9 Nov week ran.
        let late = done_at(
            due(task(1, "Late report"), date(2026, 11, 12)),
            "2026-11-16T10:00:00Z",
        );
        let events = vec![status(
            1,
            1,
            "2026-11-16T10:00:00Z",
            TaskStatus::Todo,
            TaskStatus::Done,
        )];
        let today = date(2026, 11, 16);
        let f = summary_facts(&[late], &events, summary_week(), today, &TimeZone::UTC);
        assert!(f.completed.is_empty());
        assert_eq!(f.missed.len(), 1, "{f:?}");
        assert_eq!(f.missed[0].title, "Late report");

        // Completed in the week and reopened later: still completed that week.
        let reopened = due(task(2, "Reopened"), date(2026, 11, 20));
        let events = vec![
            status(
                1,
                2,
                "2026-11-11T10:00:00Z",
                TaskStatus::Todo,
                TaskStatus::Done,
            ),
            status(
                2,
                2,
                "2026-11-17T10:00:00Z",
                TaskStatus::Done,
                TaskStatus::Todo,
            ),
        ];
        let f = summary_facts(&[reopened], &events, summary_week(), today, &TimeZone::UTC);
        assert_eq!(f.completed.len(), 1);
        assert_eq!(f.completed[0].completed_on, date(2026, 11, 11));

        // Created after the week: not part of it.
        let new = created(
            due(task(3, "Brand new"), date(2026, 11, 13)),
            "2026-11-16T09:00:00Z",
        );
        let f = summary_facts(&[new], &[], summary_week(), today, &TimeZone::UTC);
        assert!(f.missed.is_empty());

        // Added during the week, finished after it: added as todo.
        let added = done_at(
            created(task(4, "Added"), "2026-11-10T09:00:00Z"),
            "2026-11-17T10:00:00Z",
        );
        let events = vec![status(
            1,
            4,
            "2026-11-17T10:00:00Z",
            TaskStatus::Todo,
            TaskStatus::Done,
        )];
        let f = summary_facts(&[added], &events, summary_week(), today, &TimeZone::UTC);
        assert_eq!(f.added.len(), 1);
        assert_eq!(f.added[0].status, TaskStatus::Todo);
        assert!(!summary_facts_markdown(&f).contains("(done)"));

        // Worked on during the week, deadline moved after it: progressed
        // with the deadline it had then.
        let moved = due(task(5, "Moved"), date(2026, 11, 27));
        let events = vec![
            log(1, Some(5), "2026-11-12T10:00:00Z", "Drafted"),
            due_moved(
                2,
                5,
                "2026-11-16T10:00:00Z",
                Some(date(2026, 11, 20)),
                Some(date(2026, 11, 27)),
            ),
        ];
        let f = summary_facts(&[moved], &events, summary_week(), today, &TimeZone::UTC);
        assert_eq!(f.progressed.len(), 1);
        assert_eq!(f.progressed[0].due_on, Some(date(2026, 11, 20)));
        assert_eq!(f.progressed[0].due_weekday, Some("Friday"));
    }

    #[test]
    fn a_deadline_moved_and_moved_back_did_not_slip() {
        let t = due(task(1, "Report"), date(2026, 11, 20));
        let events = vec![
            due_moved(
                1,
                1,
                "2026-11-10T09:00:00Z",
                Some(date(2026, 11, 20)),
                Some(date(2026, 11, 23)),
            ),
            due_moved(
                2,
                1,
                "2026-11-10T09:01:00Z",
                Some(date(2026, 11, 23)),
                Some(date(2026, 11, 20)),
            ),
        ];
        assert!(
            summary_of(std::slice::from_ref(&t), &events)
                .slipped
                .is_empty()
        );
        let plan = plan_facts(
            &[t],
            &events,
            WeekRef::new(date(2026, 11, 16)),
            date(2026, 11, 16),
            &Settings::default(),
        );
        let all: Vec<&PlanTask> = plan
            .overdue
            .iter()
            .chain(&plan.due_this_week)
            .chain(&plan.due_soon)
            .chain(&plan.undated)
            .collect();
        assert_eq!(all[0].deadline_moves, 0);
    }

    #[test]
    fn events_near_midnight_follow_the_local_date() {
        // 23:30 UTC on Sunday the 15th is Monday the 16th in UTC+2, and
        // 23:30 UTC on Sunday the 8th is Monday the 9th there.
        let tasks = vec![task(1, "Task")];
        let events = vec![
            log(1, None, "2026-11-08T23:30:00Z", "Early"),
            log(2, None, "2026-11-15T23:30:00Z", "Late"),
            log(3, Some(1), "2026-11-15T23:30:00Z", "Task late"),
            // 03:00 UTC on Monday the 16th is Sunday 22:00 in UTC-5.
            log(4, None, "2026-11-16T03:00:00Z", "Western"),
        ];
        let texts = |zone: &TimeZone| {
            let f = summary_facts(&tasks, &events, summary_week(), summary_week().end(), zone);
            let free: Vec<String> = f.log_lines.iter().map(|l| l.text.clone()).collect();
            (free, f.progressed.len())
        };
        assert_eq!(texts(&TimeZone::UTC), (vec!["Late".to_string()], 1));
        assert_eq!(
            texts(&TimeZone::fixed(tz::offset(2))),
            (vec!["Early".to_string()], 0)
        );
        let west = TimeZone::fixed(tz::offset(-5));
        let (free, progressed) = texts(&west);
        assert_eq!(free, vec!["Late", "Western"]);
        assert_eq!(progressed, 1);
        let f = summary_facts(&tasks, &events, summary_week(), summary_week().end(), &west);
        assert_eq!(f.log_lines[1].date, date(2026, 11, 15));
        assert_eq!(f.log_lines[1].weekday, "Sunday");
    }

    #[test]
    fn summary_instructions_are_appended() {
        let mut s = Settings::default();
        let p = prepare_summary(
            &[],
            &[],
            summary_week(),
            summary_week().end(),
            &TimeZone::UTC,
            &s,
        );
        assert!(!p.request.system.contains("preferences"));
        s.summary_instructions = "  Under 150 words. Friendly.  ".into();
        let p = prepare_summary(
            &[],
            &[],
            summary_week(),
            summary_week().end(),
            &TimeZone::UTC,
            &s,
        );
        assert!(p.request.system.ends_with("\nUnder 150 words. Friendly."));
        assert_eq!(p.kind, ReportKind::Summary);
    }

    #[test]
    fn snapshot_summary() {
        let tasks = summary_fixture();
        let events = summary_events();
        let facts = summary_of(&tasks, &events);
        let settings = Settings {
            summary_instructions: "Keep it under 200 words.".into(),
            ..Settings::default()
        };
        let p = prepare_summary(
            &tasks,
            &events,
            summary_week(),
            summary_week().end(),
            &TimeZone::UTC,
            &settings,
        );
        snap_settings().bind(|| {
            insta::assert_json_snapshot!("report__summary_facts", facts);
            insta::assert_snapshot!("report__summary_markdown", summary_facts_markdown(&facts));
            insta::assert_snapshot!("report__summary_system", p.request.system);
            insta::assert_snapshot!("report__summary_user", p.request.user);
        });
    }

    #[test]
    fn snapshot_empty_week() {
        let summary = summary_of(&[], &[]);
        snap_settings().bind(|| {
            insta::assert_json_snapshot!("report__empty_summary_facts", summary);
            insta::assert_snapshot!(
                "report__empty_summary_markdown",
                summary_facts_markdown(&summary)
            );
        });
    }
}
