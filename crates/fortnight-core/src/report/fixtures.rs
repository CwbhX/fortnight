//! Test builders and fixtures shared by the report tests.

use super::summary::*;
use crate::domain::{Event, EventKind, Task, TaskId, TaskStatus, WeekRef};
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::civil::date;
use jiff::tz::TimeZone;

pub(super) fn ts(s: &str) -> Timestamp {
    s.parse().expect("timestamp")
}

pub(super) fn task(id: i64, title: &str) -> Task {
    Task {
        id,
        title: title.into(),
        notes: String::new(),
        status: TaskStatus::Todo,
        project: None,
        due_on: None,
        position: id as f64,
        created_at: ts("2026-10-01T09:00:00Z"),
        updated_at: ts("2026-10-01T09:00:00Z"),
        completed_at: None,
        ..Task::default()
    }
}

pub(super) fn project(mut t: Task, p: &str) -> Task {
    t.project = Some(p.into());
    t
}

/// Due on `d`, and planned then too unless already planned (as
/// migration 3 leaves older tasks).
pub(super) fn due(mut t: Task, d: Date) -> Task {
    t.due_on = Some(d);
    t.planned_for = t.planned_for.or(Some(d));
    t
}

pub(super) fn done_at(mut t: Task, at: &str) -> Task {
    t.status = TaskStatus::Done;
    t.completed_at = Some(ts(at));
    t.updated_at = ts(at);
    t
}

pub(super) fn created(mut t: Task, at: &str) -> Task {
    t.created_at = ts(at);
    t.updated_at = ts(at);
    t
}

pub(super) fn updated(mut t: Task, at: &str) -> Task {
    t.updated_at = ts(at);
    t
}

pub(super) fn estimate(mut t: Task, minutes: u32) -> Task {
    t.estimate_minutes = Some(minutes);
    t
}

pub(super) fn planned(mut t: Task, d: Date) -> Task {
    t.planned_for = Some(d);
    t
}

pub(super) fn private(mut t: Task) -> Task {
    t.private = true;
    t
}

pub(super) fn ev(id: i64, task_id: Option<TaskId>, at: &str, kind: EventKind) -> Event {
    Event {
        id,
        task_id,
        at: ts(at),
        kind,
    }
}

pub(super) fn log(id: i64, task_id: Option<TaskId>, at: &str, text: &str) -> Event {
    ev(id, task_id, at, EventKind::Log { text: text.into() })
}

pub(super) fn status(
    id: i64,
    task_id: TaskId,
    at: &str,
    from: TaskStatus,
    to: TaskStatus,
) -> Event {
    ev(id, Some(task_id), at, EventKind::StatusChanged { from, to })
}

pub(super) fn due_moved(
    id: i64,
    task_id: TaskId,
    at: &str,
    from: Option<Date>,
    to: Option<Date>,
) -> Event {
    ev(id, Some(task_id), at, EventKind::DueChanged { from, to })
}

pub(super) fn edited(id: i64, task_id: TaskId, at: &str) -> Event {
    ev(
        id,
        Some(task_id),
        at,
        EventKind::Edited {
            fields: vec!["notes".into()],
        },
    )
}

/// Week behind: Monday 2026-11-09 to Sunday 2026-11-15.
pub(super) fn summary_week() -> WeekRef {
    WeekRef::new(date(2026, 11, 9))
}

pub(super) fn summary_of(tasks: &[Task], events: &[Event]) -> SummaryFacts {
    summary_facts(
        tasks,
        events,
        summary_week(),
        summary_week().end(),
        &TimeZone::UTC,
    )
}

pub(super) fn summary_fixture() -> Vec<Task> {
    let mut login = project(task(1, "Ship login page"), "Website");
    login.notes = "Released behind the `new-login` flag; 5% of users so far.".into();
    let mut vendor = due(
        project(task(6, "Vendor contract review"), "Finance"),
        date(2026, 11, 12),
    );
    vendor.notes = "Waiting on legal <legal@example.com> to reply.".into();
    let mut reopened = due(
        updated(
            project(task(5, "Migrate analytics"), "Website"),
            "2026-11-12T10:00:00Z",
        ),
        date(2026, 11, 20),
    );
    reopened.notes = "Reopened: the dashboard numbers were off.".into();
    let mut old_plan = project(task(13, "Old migration plan"), "Website");
    old_plan.status = TaskStatus::Dropped;
    old_plan.notes = "Superseded by the analytics migration.".into();
    vec![
        done_at(login, "2026-11-11T14:00:00Z"),
        done_at(task(2, "Fix flaky CI job"), "2026-11-13T16:30:00Z"),
        // Completed just before and just after the week.
        done_at(
            project(task(3, "Q4 budget draft"), "Finance"),
            "2026-11-08T23:59:59Z",
        ),
        done_at(task(4, "Expense report"), "2026-11-16T00:00:00Z"),
        reopened,
        vendor,
        due(
            project(task(7, "Write onboarding doc"), "Website"),
            date(2026, 11, 15),
        ),
        created(task(8, "Plan team offsite"), "2026-11-10T08:00:00Z"),
        updated(
            project(task(9, "Hiring rubric"), "Hiring"),
            "2026-11-13T11:00:00Z",
        ),
        done_at(
            created(
                project(task(10, "Book venue"), "Hiring"),
                "2026-11-10T09:00:00Z",
            ),
            "2026-11-12T09:00:00Z",
        ),
        due(task(11, "Renew certificates"), date(2026, 11, 22)),
        // Due beyond next week: not in next_week.
        due(task(12, "Annual review"), date(2026, 11, 30)),
        old_plan,
        // Worked on this week, finished the week after.
        done_at(
            project(task(14, "Q3 retro write-up"), "Team"),
            "2026-11-17T15:00:00Z",
        ),
        // Private: must never appear.
        done_at(
            private(project(task(15, "Interview with Acme Corp"), "Personal")),
            "2026-11-12T12:00:00Z",
        ),
    ]
}

pub(super) fn summary_events() -> Vec<Event> {
    let (todo, doing, done, dropped) = (
        TaskStatus::Todo,
        TaskStatus::Doing,
        TaskStatus::Done,
        TaskStatus::Dropped,
    );
    vec![
        // Before the week: counts for nothing in this week.
        due_moved(
            1,
            6,
            "2026-11-04T09:00:00Z",
            Some(date(2026, 11, 6)),
            Some(date(2026, 11, 10)),
        ),
        log(2, Some(9), "2026-11-05T09:00:00Z", "Collected old rubrics"),
        // In the week.
        due_moved(
            3,
            6,
            "2026-11-09T09:00:00Z",
            Some(date(2026, 11, 10)),
            Some(date(2026, 11, 11)),
        ),
        status(4, 5, "2026-11-10T09:00:00Z", todo, doing),
        log(
            5,
            None,
            "2026-11-10T11:00:00Z",
            "Helped Sam debug the deploy",
        ),
        log(
            6,
            Some(1),
            "2026-11-11T10:00:00Z",
            "Rolled out to 5% of users",
        ),
        log(
            7,
            Some(9),
            "2026-11-11T15:00:00Z",
            "Drafted the scoring scale",
        ),
        due_moved(
            8,
            6,
            "2026-11-11T16:00:00Z",
            Some(date(2026, 11, 11)),
            Some(date(2026, 11, 12)),
        ),
        status(9, 1, "2026-11-11T14:00:00Z", doing, done),
        status(10, 13, "2026-11-12T09:00:00Z", todo, dropped),
        log(
            11,
            Some(5),
            "2026-11-12T10:00:00Z",
            "Found the double-counting bug",
        ),
        log(
            12,
            Some(15),
            "2026-11-12T11:00:00Z",
            "Prepared Acme answers",
        ),
        status(13, 15, "2026-11-12T12:00:00Z", todo, done),
        log(14, Some(14), "2026-11-13T10:00:00Z", "Wrote the first half"),
        edited(15, 9, "2026-11-13T11:00:00Z"),
        log(16, None, "2026-11-14T09:00:00Z", "Ran the team retro"),
        // After the week.
        log(17, None, "2026-11-16T09:00:00Z", "Next week's log line"),
        status(18, 14, "2026-11-17T15:00:00Z", todo, done),
    ]
}

/// Week ahead: Monday 2026-11-16 to Sunday 2026-11-22, today Wednesday 18.
pub(super) fn plan_week() -> WeekRef {
    WeekRef::new(date(2026, 11, 16))
}

pub(super) fn plan_today() -> Date {
    date(2026, 11, 18)
}

pub(super) fn plan_fixture() -> Vec<Task> {
    let mut deck = due(
        project(task(3, "Board deck"), "Finance"),
        date(2026, 11, 20),
    );
    deck.notes = "Needs Q3 actuals from Sam.".into();
    deck.priority = 2;
    vec![
        estimate(
            due(
                project(task(1, "Vendor contract review"), "Finance"),
                date(2026, 11, 12),
            ),
            60,
        ),
        estimate(
            due(
                project(task(2, "Write onboarding doc"), "Website"),
                date(2026, 11, 18),
            ),
            120,
        ),
        estimate(planned(deck, date(2026, 11, 19)), 240),
        due(task(4, "Renew certificates"), date(2026, 11, 22)),
        due(
            project(task(5, "Migrate analytics"), "Website"),
            date(2026, 11, 25),
        ),
        due(task(6, "Annual review"), date(2026, 12, 2)),
        // 15 days out: beyond the look-ahead.
        due(task(7, "Conference talk"), date(2026, 12, 3)),
        project(task(8, "Hiring rubric"), "Hiring"),
        task(9, "Plan team offsite"),
        done_at(
            due(task(10, "Already done"), date(2026, 11, 19)),
            "2026-11-17T10:00:00Z",
        ),
        // Due far out but planned for Friday: listed, and in Friday's load.
        estimate(
            planned(
                due(task(11, "Draft 2027 roadmap"), date(2026, 12, 15)),
                date(2026, 11, 20),
            ),
            90,
        ),
        // Private: never in the plan, but in `day_loads`.
        estimate(
            private(due(task(12, "Doctor appointment"), date(2026, 11, 19))),
            120,
        ),
    ]
}

pub(super) fn plan_events() -> Vec<Event> {
    vec![
        due_moved(
            1,
            3,
            "2026-11-02T09:00:00Z",
            Some(date(2026, 11, 13)),
            Some(date(2026, 11, 18)),
        ),
        due_moved(
            2,
            3,
            "2026-11-16T09:00:00Z",
            Some(date(2026, 11, 18)),
            Some(date(2026, 11, 20)),
        ),
        // Earlier, set and cleared: not moves later.
        due_moved(
            3,
            4,
            "2026-11-16T09:00:00Z",
            Some(date(2026, 11, 25)),
            Some(date(2026, 11, 22)),
        ),
        due_moved(4, 2, "2026-11-10T09:00:00Z", None, Some(date(2026, 11, 18))),
        due_moved(5, 5, "2026-11-10T09:00:00Z", Some(date(2026, 11, 20)), None),
        due_moved(
            6,
            1,
            "2026-11-05T09:00:00Z",
            Some(date(2026, 11, 9)),
            Some(date(2026, 11, 12)),
        ),
        // Private task's move.
        due_moved(
            7,
            12,
            "2026-11-05T09:00:00Z",
            Some(date(2026, 11, 9)),
            Some(date(2026, 11, 19)),
        ),
    ]
}

pub(super) fn snap_settings() -> insta::Settings {
    let mut s = insta::Settings::clone_current();
    s.set_prepend_module_to_snapshot(false);
    s
}
