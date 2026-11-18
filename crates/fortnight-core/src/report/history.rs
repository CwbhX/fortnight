//! Task history: which events and tasks may be shared, and a task's
//! state at the end of a week.

use super::format::*;
use super::summary::*;
use crate::domain::{Event, EventKind, Task, TaskId, TaskStatus, WeekRef};
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use std::collections::{BTreeMap, BTreeSet};

/// The events of the tasks that may be shared (not private), grouped by
/// task, plus free-standing log lines. Oldest first within each list.
pub(super) struct History<'a> {
    pub(super) by_task: BTreeMap<TaskId, Vec<&'a Event>>,
    pub(super) free_logs: Vec<&'a Event>,
}

impl<'a> History<'a> {
    /// Keeps only events of `tasks` (already filtered to shareable ones)
    /// and free-standing log lines. Events of private or unknown tasks are
    /// dropped.
    pub(super) fn new(tasks: &[&Task], events: &'a [Event]) -> Self {
        let ids: BTreeSet<TaskId> = tasks.iter().map(|t| t.id).collect();
        let mut sorted: Vec<&Event> = events.iter().collect();
        sorted.sort_by_key(|e| (e.at, e.id));
        let mut by_task: BTreeMap<TaskId, Vec<&Event>> = BTreeMap::new();
        let mut free_logs = Vec::new();
        for e in sorted {
            match e.task_id {
                Some(id) if ids.contains(&id) => by_task.entry(id).or_default().push(e),
                Some(_) => {}
                None if e.kind.is_log() => free_logs.push(e),
                None => {}
            }
        }
        History { by_task, free_logs }
    }

    pub(super) fn of(&self, id: TaskId) -> &[&'a Event] {
        self.by_task.get(&id).map_or(&[], Vec::as_slice)
    }
}

/// Local dates for one week in one time zone.
pub(super) struct LocalWeek<'a> {
    pub(super) week: WeekRef,
    pub(super) tz: &'a TimeZone,
}

impl LocalWeek<'_> {
    pub(super) fn date(&self, ts: Timestamp) -> Date {
        self.tz.to_datetime(ts).date()
    }

    /// The event's local date falls in the week.
    pub(super) fn has(&self, e: &Event) -> bool {
        self.week.contains(self.date(e.at))
    }

    /// The task's events in the week, oldest first.
    pub(super) fn events<'e>(&self, history: &[&'e Event]) -> Vec<&'e Event> {
        history.iter().copied().filter(|e| self.has(e)).collect()
    }

    /// The task's log lines in the week, oldest first.
    pub(super) fn logs(&self, history: &[&Event]) -> Vec<LogEntry> {
        history
            .iter()
            .filter(|e| self.has(e))
            .filter_map(|e| self.log_entry(e))
            .collect()
    }

    pub(super) fn log_entry(&self, e: &Event) -> Option<LogEntry> {
        let text = log_text(e.log_text()?);
        if text.is_empty() {
            return None;
        }
        let date = self.date(e.at);
        Some(LogEntry {
            date,
            weekday: weekday_name(date.weekday()),
            text,
        })
    }

    /// The task's status at the end of the week: the `to` of its last
    /// status change on or before the week's last day; otherwise the `from`
    /// of its first later change; otherwise its current status (no history).
    pub(super) fn status_at_end(&self, task: &Task, history: &[&Event]) -> TaskStatus {
        let end = self.week.end();
        let mut before = None;
        let mut after = None;
        for e in history {
            if let EventKind::StatusChanged { from, to } = e.kind {
                if self.date(e.at) <= end {
                    before = Some(to);
                } else if after.is_none() {
                    after = Some(from);
                }
            }
        }
        before.or(after).unwrap_or(task.status)
    }

    /// The task's deadline at the end of the week, by the same rule as
    /// `status_at_end`.
    pub(super) fn due_at_end(&self, task: &Task, history: &[&Event]) -> Option<Date> {
        let end = self.week.end();
        let mut before = None;
        let mut after = None;
        for e in history {
            if let EventKind::DueChanged { from, to } = e.kind {
                if self.date(e.at) <= end {
                    before = Some(to);
                } else if after.is_none() {
                    after = Some(from);
                }
            }
        }
        before.or(after).unwrap_or(task.due_on)
    }

    /// When the task was completed in this week: it was `Done` at the end of
    /// the week and its last change to `Done` falls in the week. Without
    /// status history, the stored `completed_at`.
    pub(super) fn completed_in_week(&self, task: &Task, history: &[&Event]) -> Option<Timestamp> {
        let end = self.week.end();
        let has_status_history = history
            .iter()
            .any(|e| matches!(e.kind, EventKind::StatusChanged { .. }));
        if !has_status_history {
            let ts = task.completed_at.filter(|_| task.is_done())?;
            return self.week.contains(self.date(ts)).then_some(ts);
        }
        if self.status_at_end(task, history) != TaskStatus::Done {
            return None;
        }
        history
            .iter()
            .filter(|e| self.date(e.at) <= end)
            .rev()
            .find(|e| {
                matches!(
                    e.kind,
                    EventKind::StatusChanged {
                        to: TaskStatus::Done,
                        ..
                    }
                )
            })
            .map(|e| e.at)
            .filter(|at| self.week.contains(self.date(*at)))
    }

    /// The task existed by the end of the week.
    pub(super) fn existed(&self, task: &Task) -> bool {
        self.date(task.created_at) <= self.week.end()
    }
}

/// `Some((from, to))` when the event moved a deadline later.
pub(super) fn later_move(kind: &EventKind) -> Option<(Date, Date)> {
    match *kind {
        EventKind::DueChanged {
            from: Some(from),
            to: Some(to),
        } if to > from => Some((from, to)),
        _ => None,
    }
}

/// How many times the deadline moved later, net of moves back earlier (an
/// undone move does not count).
pub(super) fn net_deadline_moves(history: &[&Event]) -> usize {
    let mut later = 0usize;
    let mut earlier = 0usize;
    for e in history {
        if let EventKind::DueChanged {
            from: Some(from),
            to: Some(to),
        } = e.kind
        {
            if to > from {
                later += 1;
            } else if to < from {
                earlier += 1;
            }
        }
    }
    later.saturating_sub(earlier)
}

/// Tasks that may be sent: every task not marked private and not deleted
/// (the app's Delete moves a task to `Dropped`, its trash).
pub(super) fn shareable(tasks: &[Task]) -> Vec<&Task> {
    tasks
        .iter()
        .filter(|t| !t.private && t.status != TaskStatus::Dropped)
        .collect()
}

/// Deadline first (undated last), then list order.
pub(super) fn sort_by_due(tasks: &mut [&Task]) {
    tasks.sort_by(|a, b| {
        let key = |t: &Task| (t.due_on.is_none(), t.due_on, t.position, t.id);
        key(a)
            .partial_cmp(&key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}
