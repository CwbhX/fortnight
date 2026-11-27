//! The commands against an in-memory store and the mock LLM client.

use fortnight_cli::args::{EditArgs, ListArgs};
use fortnight_cli::commands::{self, now_for};
use fortnight_cli::error::{CliError, CliResult};
use fortnight_cli::llm_setup::LlmHandle;
use fortnight_cli::run::lock_warning;
use fortnight_core::datadir;
use fortnight_core::domain::{
    NewTask, Recurrence, ReportKind, ReportStatus, Task, TaskStatus, WeekRef,
};
use fortnight_core::export::{self, ExportFormat};
use fortnight_core::fallback::Endpoint;
use fortnight_core::llm::{LlmError, MockClient};
use fortnight_core::sqlite::SqliteStore;
use fortnight_core::store::Store;
use jiff::Timestamp;
use jiff::civil::{Date, Weekday, date};
use jiff::tz::TimeZone;
use std::sync::Arc;

// Monday 16 November 2026 starts the week; Wednesday the 18th is "today".
fn wednesday() -> Date {
    date(2026, 11, 18)
}

/// The tests' weeks start on Monday (the default is Sunday).
fn monday_settings() -> fortnight_core::domain::Settings {
    let mut settings = fortnight_core::domain::Settings::default();
    settings.schedule.week_start = Weekday::Monday;
    settings
}

fn store() -> SqliteStore {
    let s = SqliteStore::open_in_memory().unwrap();
    s.save_settings(&monday_settings()).unwrap();
    s
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

fn add_task(store: &SqliteStore, title: &str, due: Option<Date>) -> Task {
    store
        .add_task(NewTask {
            title: title.into(),
            due_on: due,
            planned_for: due,
            ..NewTask::default()
        })
        .unwrap()
}

fn text(out: Vec<u8>) -> String {
    String::from_utf8(out).unwrap()
}

fn handle(mock: &Arc<MockClient>) -> LlmHandle {
    LlmHandle {
        main: Endpoint {
            client: mock.clone(),
            host: Some("mock.test".into()),
        },
        decision: None,
    }
}

fn now(day: Date) -> jiff::ZonedDateTime {
    now_for(Some(day), TimeZone::UTC).unwrap()
}

// ---------------------------------------------------------------- add

#[test]
fn add_parses_quick_add_and_prints_id_and_summary() {
    let s = store();
    let mut out = Vec::new();
    commands::add(
        &s,
        &words("write report fri #work ~2h !!"),
        false,
        wednesday(),
        &mut out,
    )
    .unwrap();
    let out = text(out);
    assert_eq!(
        out,
        "Added 1: write report (on 2026-11-20, #work, ~2h, !!)\n"
    );
    let t = s.get_task(1).unwrap().unwrap();
    assert_eq!(t.title, "write report");
    assert_eq!(t.planned_for, Some(date(2026, 11, 20)));
    assert_eq!(t.due_on, None);
    assert_eq!(t.project.as_deref(), Some("work"));
    assert_eq!(t.estimate_minutes, Some(120));
    assert_eq!(t.priority, 2);
    assert!(!t.private);

    // When `due_on_added_day` is enabled, quick-add sets `due_on` to the
    // planned day too, matching the app.
    let mut settings = monday_settings();
    settings.due_on_added_day = true;
    s.save_settings(&settings).unwrap();
    commands::add(
        &s,
        &words("file taxes fri"),
        false,
        wednesday(),
        &mut Vec::new(),
    )
    .unwrap();
    let t2 = s.get_task(2).unwrap().unwrap();
    assert_eq!(t2.planned_for, Some(date(2026, 11, 20)));
    assert_eq!(t2.due_on, Some(date(2026, 11, 20)));
}

#[test]
fn add_private_due_and_recurrence() {
    let s = store();
    let mut out = Vec::new();
    commands::add(
        &s,
        &words("review budget today due:fri every:weekly #finance"),
        true,
        wednesday(),
        &mut out,
    )
    .unwrap();
    assert_eq!(
        text(out),
        "Added 1: review budget (on 2026-11-18, due 2026-11-20, ↻weekly, #finance, private)\n"
    );
    let t = s.get_task(1).unwrap().unwrap();
    assert!(t.private);
    assert_eq!(t.planned_for, Some(date(2026, 11, 18)));
    assert_eq!(t.due_on, Some(date(2026, 11, 20)));
    assert_eq!(t.recurrence, Some(Recurrence::Weekly));
    assert_eq!(t.project.as_deref(), Some("finance"));
}

#[test]
fn add_plain_title_and_title_required() {
    let s = store();
    let mut out = Vec::new();
    commands::add(&s, &words("buy milk"), false, wednesday(), &mut out).unwrap();
    assert_eq!(text(out), "Added 1: buy milk\n");
    let err =
        commands::add(&s, &words("fri #work"), false, wednesday(), &mut Vec::new()).unwrap_err();
    assert_eq!(err.code, 2);
    assert_eq!(s.list_tasks().unwrap().len(), 1);
}

// --------------------------------------------------------------- list

#[test]
fn list_orders_overdue_then_day_then_undated_and_hides_closed() {
    let s = store();
    let undated = add_task(&s, "undated", None);
    let later = add_task(&s, "later", Some(date(2026, 11, 25)));
    let today = add_task(&s, "today", Some(wednesday()));
    let overdue_b = add_task(&s, "overdue b", Some(date(2026, 11, 16)));
    let overdue_a = add_task(&s, "overdue a", Some(date(2026, 11, 10)));
    let done = add_task(&s, "finished", Some(wednesday()));
    s.set_status(done.id, TaskStatus::Done).unwrap();
    let dropped = add_task(&s, "dropped", None);
    s.set_status(dropped.id, TaskStatus::Dropped).unwrap();

    let mut out = Vec::new();
    commands::list(&s, &ListArgs::default(), wednesday(), &mut out).unwrap();
    let out = text(out);
    let order: Vec<i64> = out
        .lines()
        .skip(1)
        .map(|l| l.split_whitespace().next().unwrap().parse().unwrap())
        .collect();
    assert_eq!(
        order,
        vec![overdue_a.id, overdue_b.id, today.id, later.id, undated.id],
        "{out}"
    );
    assert!(!out.contains("finished"));

    let mut all = Vec::new();
    let opts = ListArgs {
        all: true,
        ..ListArgs::default()
    };
    commands::list(&s, &opts, wednesday(), &mut all).unwrap();
    let all = text(all);
    assert!(all.contains("finished"));
    assert!(all.contains("dropped"));
}

#[test]
fn list_columns_are_aligned_and_complete() {
    let s = store();
    s.add_task(NewTask {
        title: "short".into(),
        project: Some("p".into()),
        due_on: Some(date(2026, 11, 20)),
        estimate_minutes: Some(90),
        priority: 3,
        ..NewTask::default()
    })
    .unwrap();
    let longer = s
        .add_task(NewTask {
            title: "a much longer title".into(),
            due_on: Some(date(2026, 11, 21)),
            ..NewTask::default()
        })
        .unwrap();
    s.set_status(longer.id, TaskStatus::Doing).unwrap();
    let mut out = Vec::new();
    commands::list(&s, &ListArgs::default(), wednesday(), &mut out).unwrap();
    let out = text(out);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("ID"));
    for col in ["ST", "TITLE", "PROJECT", "DUE", "EST", "PRI"] {
        assert!(lines[0].contains(col), "{}", lines[0]);
    }
    // Every row starts the due date at the same column.
    let col = lines[0].find("DUE").unwrap();
    assert_eq!(lines[1].find("2026-11-20"), Some(col), "{out}");
    assert_eq!(lines[2].find("2026-11-21"), Some(col), "{out}");
    assert!(lines[1].contains("[ ]") && lines[1].contains("1h30m") && lines[1].ends_with("!!!"));
    assert!(lines[2].contains("[~]"));
}

#[test]
fn list_today_and_week_filters_keep_overdue() {
    let s = store();
    add_task(&s, "overdue", Some(date(2026, 11, 2)));
    add_task(&s, "today", Some(wednesday()));
    add_task(&s, "friday", Some(date(2026, 11, 20)));
    add_task(&s, "next week", Some(date(2026, 11, 24)));
    add_task(&s, "undated", None);
    // Planned for Monday, no deadline, still open: carried over to today.
    s.add_task(NewTask {
        title: "carried".into(),
        planned_for: Some(date(2026, 11, 16)),
        ..NewTask::default()
    })
    .unwrap();
    // Undated with a passed deadline: overdue, so listed.
    s.add_task(NewTask {
        title: "late".into(),
        due_on: Some(date(2026, 11, 17)),
        ..NewTask::default()
    })
    .unwrap();
    let closed = add_task(&s, "closed", Some(wednesday()));
    s.set_status(closed.id, TaskStatus::Done).unwrap();
    let titles = |opts: ListArgs| -> Vec<String> {
        let mut out = Vec::new();
        commands::list(&s, &opts, wednesday(), &mut out).unwrap();
        text(out)
            .lines()
            .skip(1)
            .map(|l| {
                l[l.find("] ").unwrap() + 2..]
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .to_string()
            })
            .collect()
    };
    assert_eq!(
        titles(ListArgs {
            today: true,
            ..ListArgs::default()
        }),
        ["overdue", "late", "carried", "today", "undated"]
    );
    // --all adds what was planned for today and is closed, nothing else.
    assert_eq!(
        titles(ListArgs {
            today: true,
            all: true,
            ..ListArgs::default()
        }),
        ["overdue", "late", "carried", "today", "closed", "undated"]
    );
    assert_eq!(
        titles(ListArgs {
            week: true,
            ..ListArgs::default()
        }),
        ["overdue", "late", "carried", "today", "friday"]
    );
}

#[test]
fn list_planned_day_wins_for_ordering() {
    let s = store();
    let a = add_task(&s, "due friday", Some(date(2026, 11, 20)));
    let b = add_task(&s, "due friday, planned thursday", Some(date(2026, 11, 20)));
    let mut moved = s.get_task(b.id).unwrap().unwrap();
    moved.planned_for = Some(date(2026, 11, 19));
    s.update_task(&moved).unwrap();
    let tasks = s.list_tasks().unwrap();
    let shown = commands::select_tasks(&tasks, &ListArgs::default(), wednesday(), Weekday::Monday);
    assert_eq!(shown[0].id, b.id);
    assert_eq!(shown[1].id, a.id);
}

#[test]
fn list_project_filter_is_case_insensitive_and_shows_recurrence_and_private() {
    let s = store();
    s.add_task(NewTask {
        title: "standup".into(),
        project: Some("Work".into()),
        recurrence: Some(Recurrence::Weekdays),
        private: true,
        planned_for: Some(wednesday()),
        ..NewTask::default()
    })
    .unwrap();
    s.add_task(NewTask {
        title: "retro".into(),
        project: Some("work".into()),
        recurrence: Some(Recurrence::Monthly),
        planned_for: Some(date(2026, 11, 20)),
        ..NewTask::default()
    })
    .unwrap();
    s.add_task(NewTask {
        title: "groceries".into(),
        project: Some("Home".into()),
        recurrence: Some(Recurrence::Daily),
        ..NewTask::default()
    })
    .unwrap();
    s.add_task(NewTask {
        title: "no project".into(),
        private: true,
        ..NewTask::default()
    })
    .unwrap();

    // Case-insensitive match (with or without leading #).
    let mut out = Vec::new();
    commands::list(
        &s,
        &ListArgs {
            project: Some("WORK".into()),
            ..ListArgs::default()
        },
        wednesday(),
        &mut out,
    )
    .unwrap();
    let out = text(out);
    assert!(out.contains("standup ↻weekdays (private)"), "{out}");
    assert!(out.contains("retro ↻monthly"), "{out}");
    assert!(!out.contains("groceries"), "{out}");
    assert!(!out.contains("no project"), "{out}");

    // Leading `#` is accepted too.
    let mut out = Vec::new();
    commands::list(
        &s,
        &ListArgs {
            project: Some("#home".into()),
            ..ListArgs::default()
        },
        wednesday(),
        &mut out,
    )
    .unwrap();
    let out = text(out);
    assert!(out.contains("groceries ↻daily"), "{out}");
    assert!(!out.contains("standup"), "{out}");

    // Non-matching project gives empty list.
    let mut out = Vec::new();
    commands::list(
        &s,
        &ListArgs {
            project: Some("unknown".into()),
            ..ListArgs::default()
        },
        wednesday(),
        &mut out,
    )
    .unwrap();
    assert_eq!(text(out), "No tasks.\n");

    // Full list shows `(private)` alone without recurrence.
    let mut out = Vec::new();
    commands::list(&s, &ListArgs::default(), wednesday(), &mut out).unwrap();
    let out = text(out);
    assert!(out.contains("no project (private)"), "{out}");
}

#[test]
fn empty_list_says_so() {
    let mut out = Vec::new();
    commands::list(&store(), &ListArgs::default(), wednesday(), &mut out).unwrap();
    assert_eq!(text(out), "No tasks.\n");
}

// -------------------------------------------------- done, delete, edit, log

#[test]
fn done_and_undo() {
    let s = store();
    let a = add_task(&s, "alpha", None);
    let b = add_task(&s, "beta", None);
    let mut out = Vec::new();
    commands::done(&s, &[a.id, b.id], false, wednesday(), &mut out).unwrap();
    assert_eq!(text(out), "Done 1: alpha\nDone 2: beta\n");
    assert!(s.get_task(a.id).unwrap().unwrap().is_done());
    assert!(s.get_task(a.id).unwrap().unwrap().completed_at.is_some());

    let mut out = Vec::new();
    commands::done(&s, &[a.id], true, wednesday(), &mut out).unwrap();
    assert_eq!(text(out), "Reopened 1: alpha\n");
    let t = s.get_task(a.id).unwrap().unwrap();
    assert_eq!(t.status, TaskStatus::Todo);
    assert!(t.completed_at.is_none());
    assert!(s.get_task(b.id).unwrap().unwrap().is_done());
}

#[test]
fn done_reports_missing_ids_but_handles_the_rest() {
    let s = store();
    let a = add_task(&s, "alpha", None);
    let mut out = Vec::new();
    let err = commands::done(&s, &[99, a.id], false, wednesday(), &mut out).unwrap_err();
    assert_eq!(err.code, 1);
    assert!(err.message.contains("99"));
    assert!(s.get_task(a.id).unwrap().unwrap().is_done());
}

#[test]
fn done_spawns_next_occurrence_for_recurring_task_idempotently() {
    let s = store();
    // Friday 2026-11-20 with weekdays recurrence -> next occurrence is Monday 2026-11-23.
    let friday = date(2026, 11, 20);
    let t = s
        .add_task(NewTask {
            title: "daily standup".into(),
            project: Some("ops".into()),
            planned_for: Some(friday),
            due_on: Some(date(2026, 11, 21)),
            estimate_minutes: Some(15),
            priority: 1,
            private: true,
            recurrence: Some(Recurrence::Weekdays),
            ..NewTask::default()
        })
        .unwrap();

    let mut out = Vec::new();
    commands::done(&s, &[t.id], false, friday, &mut out).unwrap();
    assert_eq!(
        text(out),
        "Done 1: daily standup\nNext occurrence #2 planned for 2026-11-23\n"
    );

    let tasks = s.list_tasks().unwrap();
    assert_eq!(tasks.len(), 2);
    let next = s.get_task(2).unwrap().unwrap();
    assert_eq!(next.title, "daily standup");
    assert_eq!(next.status, TaskStatus::Todo);
    assert_eq!(next.planned_for, Some(date(2026, 11, 23)));
    assert_eq!(next.due_on, Some(date(2026, 11, 24)));
    assert_eq!(next.project.as_deref(), Some("ops"));
    assert_eq!(next.estimate_minutes, Some(15));
    assert_eq!(next.priority, 1);
    assert!(next.private);
    assert_eq!(next.recurrence, Some(Recurrence::Weekdays));

    // Marking already-done task done again does NOT spawn another.
    let mut out = Vec::new();
    commands::done(&s, &[t.id], false, friday, &mut out).unwrap();
    assert_eq!(text(out), "Done 1: daily standup\n");
    assert_eq!(s.list_tasks().unwrap().len(), 2);

    // Undo then re-done is idempotent (an open occurrence already exists).
    commands::done(&s, &[t.id], true, friday, &mut Vec::new()).unwrap();
    let mut out = Vec::new();
    commands::done(&s, &[t.id], false, friday, &mut out).unwrap();
    assert_eq!(text(out), "Done 1: daily standup\n");
    assert_eq!(s.list_tasks().unwrap().len(), 2);

    // A dropped recurring task marked done was not previously open -> does NOT spawn.
    let dropped = s
        .add_task(NewTask {
            title: "dropped repeat".into(),
            recurrence: Some(Recurrence::Daily),
            planned_for: Some(friday),
            ..NewTask::default()
        })
        .unwrap();
    s.set_status(dropped.id, TaskStatus::Dropped).unwrap();
    let mut out = Vec::new();
    commands::done(&s, &[dropped.id], false, friday, &mut out).unwrap();
    assert_eq!(text(out), "Done 3: dropped repeat\n");
    assert_eq!(s.list_tasks().unwrap().len(), 3);
}

#[test]
fn delete_soft_and_forever_and_missing_ids() {
    let s = store();
    let a = add_task(&s, "alpha", None);
    let b = add_task(&s, "beta", None);
    let c = add_task(&s, "gamma", None);

    // Soft-delete moves to Dropped and preserves the task in storage.
    let mut out = Vec::new();
    commands::delete(&s, &[a.id, b.id], false, &mut out).unwrap();
    assert_eq!(text(out), "Dropped 1: alpha\nDropped 2: beta\n");
    assert_eq!(
        s.get_task(a.id).unwrap().unwrap().status,
        TaskStatus::Dropped
    );
    assert_eq!(
        s.get_task(b.id).unwrap().unwrap().status,
        TaskStatus::Dropped
    );

    // Permanent delete removes task and its events.
    let mut out = Vec::new();
    commands::delete(&s, &[b.id], true, &mut out).unwrap();
    assert_eq!(text(out), "Deleted 2: beta\n");
    assert!(s.get_task(b.id).unwrap().is_none());
    assert!(s.task_events(b.id).unwrap().is_empty());

    // Missing IDs are reported while valid IDs still succeed.
    let mut out = Vec::new();
    let err = commands::delete(&s, &[99, c.id], false, &mut out).unwrap_err();
    assert_eq!(err.code, 1);
    assert!(err.message.contains("99"));
    assert_eq!(text(out), "Dropped 3: gamma\n");
    assert_eq!(
        s.get_task(c.id).unwrap().unwrap().status,
        TaskStatus::Dropped
    );

    let err = commands::delete(&s, &[99], true, &mut Vec::new()).unwrap_err();
    assert_eq!(err.code, 1);
    assert!(err.message.contains("99"));
}

#[test]
fn edit_updates_and_clears_all_fields() {
    let s = store();
    let t = add_task(&s, "original", None);

    // Set every editable field.
    let mut out = Vec::new();
    commands::edit(
        &s,
        &EditArgs {
            id: t.id,
            title: Some("  renamed task  ".into()),
            project: Some("#Eng".into()),
            due: Some("+5d".into()),
            planned: Some("fri".into()),
            estimate: Some(90),
            priority: Some(2),
            private: true,
            repeat: Some(Recurrence::Weekly),
            ..EditArgs::default()
        },
        wednesday(),
        &mut out,
    )
    .unwrap();
    assert_eq!(
        text(out),
        "Updated 1: renamed task (on 2026-11-20, due 2026-11-23, ↻weekly, #Eng, ~1h30m, !!, private)\n"
    );
    let updated = s.get_task(t.id).unwrap().unwrap();
    assert_eq!(updated.title, "renamed task");
    assert_eq!(updated.project.as_deref(), Some("Eng"));
    assert_eq!(updated.due_on, Some(date(2026, 11, 23)));
    assert_eq!(updated.planned_for, Some(date(2026, 11, 20)));
    assert_eq!(updated.estimate_minutes, Some(90));
    assert_eq!(updated.priority, 2);
    assert!(updated.private);
    assert_eq!(updated.recurrence, Some(Recurrence::Weekly));

    // Clear every clearable field and lower priority to 0.
    let mut out = Vec::new();
    commands::edit(
        &s,
        &EditArgs {
            id: t.id,
            clear_project: true,
            clear_due: true,
            clear_planned: true,
            clear_estimate: true,
            priority: Some(0),
            no_private: true,
            clear_repeat: true,
            ..EditArgs::default()
        },
        wednesday(),
        &mut out,
    )
    .unwrap();
    assert_eq!(text(out), "Updated 1: renamed task\n");
    let cleared = s.get_task(t.id).unwrap().unwrap();
    assert_eq!(cleared.project, None);
    assert_eq!(cleared.due_on, None);
    assert_eq!(cleared.planned_for, None);
    assert_eq!(cleared.estimate_minutes, None);
    assert_eq!(cleared.priority, 0);
    assert!(!cleared.private);
    assert_eq!(cleared.recurrence, None);
}

#[test]
fn edit_validates_inputs_and_missing_id() {
    let s = store();
    let t = add_task(&s, "alpha", None);

    // Date parsing covers ISO, keywords, weekdays, relative, month-day, and rejects garbage.
    assert_eq!(
        commands::parse_date_arg("2026-12-01", wednesday()).unwrap(),
        date(2026, 12, 1)
    );
    assert_eq!(
        commands::parse_date_arg("today", wednesday()).unwrap(),
        date(2026, 11, 18)
    );
    assert_eq!(
        commands::parse_date_arg("tomorrow", wednesday()).unwrap(),
        date(2026, 11, 19)
    );
    assert_eq!(
        commands::parse_date_arg("nov 25", wednesday()).unwrap(),
        date(2026, 11, 25)
    );
    assert_eq!(
        commands::parse_date_arg("+3d", wednesday()).unwrap(),
        date(2026, 11, 21)
    );
    for bad in ["", "someday", "fri #work", "2026-11-20 extra", "fri mon"] {
        let err = commands::parse_date_arg(bad, wednesday()).unwrap_err();
        assert_eq!(err.code, 2, "{bad}");
    }

    // Empty title rejected with exit code 2.
    let err = commands::edit(
        &s,
        &EditArgs {
            id: t.id,
            title: Some("   ".into()),
            ..EditArgs::default()
        },
        wednesday(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code, 2);

    // Empty project rejected with exit code 2.
    let err = commands::edit(
        &s,
        &EditArgs {
            id: t.id,
            project: Some("#".into()),
            ..EditArgs::default()
        },
        wednesday(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code, 2);

    // Bad due date rejected with exit code 2.
    let err = commands::edit(
        &s,
        &EditArgs {
            id: t.id,
            due: Some("not-a-date".into()),
            ..EditArgs::default()
        },
        wednesday(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code, 2);

    // Priority out of range rejected with exit code 2.
    let err = commands::edit(
        &s,
        &EditArgs {
            id: t.id,
            priority: Some(4),
            ..EditArgs::default()
        },
        wednesday(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code, 2);

    // Missing task id rejected with exit code 1.
    let err = commands::edit(
        &s,
        &EditArgs {
            id: 99,
            title: Some("valid".into()),
            ..EditArgs::default()
        },
        wednesday(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code, 1);
    assert!(err.message.contains("99"));
}

#[test]
fn log_lines_attach_to_tasks_or_stand_alone() {
    let s = store();
    let t = add_task(&s, "alpha", None);
    let mut out = Vec::new();
    commands::log(&s, Some(t.id), &words("sketched the first draft"), &mut out).unwrap();
    assert_eq!(text(out), "Logged on task 1.\n");
    let mut out = Vec::new();
    commands::log(&s, None, &words("met the team"), &mut out).unwrap();
    assert_eq!(text(out), "Logged.\n");

    let events = s.events_between(Timestamp::MIN, Timestamp::MAX).unwrap();
    let logs: Vec<_> = events.iter().filter(|e| e.kind.is_log()).collect();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0].task_id, Some(t.id));
    assert_eq!(logs[0].log_text(), Some("sketched the first draft"));
    assert_eq!(logs[1].task_id, None);

    let err = commands::log(&s, Some(42), &words("x"), &mut Vec::new()).unwrap_err();
    assert_eq!(err.code, 1);
    let err = commands::log(&s, None, &[], &mut Vec::new()).unwrap_err();
    assert_eq!(err.code, 2);
}

// ------------------------------------------------------------- week arg

#[test]
fn week_arguments() {
    let mon = Weekday::Monday;
    let w = |a: &str| commands::parse_week_arg(a, wednesday(), mon).map(|w| w.start);
    assert_eq!(w("this").unwrap(), date(2026, 11, 16));
    assert_eq!(w("LAST").unwrap(), date(2026, 11, 9));
    // Any date in the week snaps to its first day.
    assert_eq!(w("2026-11-26").unwrap(), date(2026, 11, 23));
    assert_eq!(w("2026-11-23").unwrap(), date(2026, 11, 23));
    let err = w("next").unwrap_err();
    assert_eq!(err.code, 2);
    assert!(w("2026-13-40").is_err());
    // Weeks starting on Sunday.
    let sun = commands::parse_week_arg("this", wednesday(), Weekday::Sunday).unwrap();
    assert_eq!(sun.start, date(2026, 11, 15));
}

#[test]
fn default_weeks() {
    let settings = monday_settings();
    let r = |kind, day: Date| {
        commands::resolve_week(kind, None, &now(day), &settings)
            .unwrap()
            .start
    };
    // Wednesday: Friday 16:00 hasn't come, so the summary is last week's.
    assert_eq!(r(ReportKind::Summary, wednesday()), date(2026, 11, 9));
    // End of Friday: this week's summary is due.
    assert_eq!(
        r(ReportKind::Summary, date(2026, 11, 20)),
        date(2026, 11, 16)
    );
    // The plan is always this week.
    assert_eq!(r(ReportKind::Plan, wednesday()), date(2026, 11, 16));
    // An explicit week wins.
    let w = commands::resolve_week(
        ReportKind::Summary,
        Some("last"),
        &now(date(2026, 11, 20)),
        &settings,
    )
    .unwrap();
    assert_eq!(w, WeekRef::new(date(2026, 11, 9)));
}

// ------------------------------------------------------------- reports

/// A task for Wednesday with a log line on Tuesday: something to
/// summarise for the week of the 16th and to recommend from Wednesday on.
fn seeded() -> SqliteStore {
    let s = store();
    let t = add_task(&s, "ship the thing", Some(wednesday()));
    let tuesday: Timestamp = "2026-11-17T10:00:00Z".parse().unwrap();
    s.add_log_at(Some(t.id), "wrote the first half", tuesday)
        .unwrap();
    s
}

#[tokio::test]
async fn summary_streams_and_stores_a_ready_report() {
    let s = seeded();
    let mock = Arc::new(MockClient::new("## Highlights\n- shipped it"));
    let mut out = Vec::new();
    commands::report_command(
        &s,
        ReportKind::Summary,
        Some("this"),
        false,
        &now(wednesday()),
        &|_| Ok(handle(&mock)),
        &mut out,
    )
    .await
    .unwrap();
    assert_eq!(text(out), "## Highlights\n- shipped it\n");

    let report = s
        .latest_report(ReportKind::Summary, date(2026, 11, 16))
        .unwrap()
        .expect("stored");
    assert_eq!(report.status, ReportStatus::Ready);
    assert_eq!(
        report.content_md.as_deref(),
        Some("## Highlights\n- shipped it")
    );
    assert_eq!(report.model.as_deref(), Some("mock"));
    assert_eq!(report.endpoint_host.as_deref(), Some("mock.test"));
    assert!(report.digest_json.contains("ship the thing"));

    // The request carried the facts, including the log line.
    let requests = mock.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].user.contains("ship the thing"));
    assert!(requests[0].user.contains("wrote the first half"));
}

#[tokio::test]
async fn plan_defaults_to_this_week_and_stores() {
    let s = seeded();
    let mock = Arc::new(MockClient::new(PLAN_REPLY));
    let mut out = Vec::new();
    commands::report_command(
        &s,
        ReportKind::Plan,
        None,
        false,
        &now(wednesday()),
        &|_| Ok(handle(&mock)),
        &mut out,
    )
    .await
    .unwrap();
    // Read back as a list of the named tasks.
    assert_eq!(text(out), "1. ship the thing\n");
    let report = s
        .latest_report(ReportKind::Plan, date(2026, 11, 16))
        .unwrap()
        .unwrap();
    assert_eq!(report.status, ReportStatus::Ready);

    // Closed, private, future, and (when disabled) overdue tasks are filtered
    // out of printed recommendations even if the model names them.
    let overdue = add_task(&s, "too late", Some(date(2026, 11, 10)));
    let future = add_task(&s, "next week", Some(date(2026, 11, 25)));
    let priv_task = s
        .add_task(NewTask {
            title: "secret".into(),
            planned_for: Some(wednesday()),
            private: true,
            ..NewTask::default()
        })
        .unwrap();
    let noisy = Arc::new(MockClient::new(format!(
        r#"{{"tasks":[{{"id":{}}},{{"id":{}}},{{"id":{}}},{{"id":1}}]}}"#,
        overdue.id, future.id, priv_task.id
    )));
    let mut out = Vec::new();
    commands::report_command(
        &s,
        ReportKind::Plan,
        None,
        false,
        &now(wednesday()),
        &|_| Ok(handle(&noisy)),
        &mut out,
    )
    .await
    .unwrap();
    assert_eq!(text(out), "1. ship the thing\n");
}

/// Replies a plan can read (`report::parse_recommendations`); a summary
/// takes any text, so these serve it too.
const PLAN_REPLY: &str = r#"{"tasks":[{"id":1,"reason":"due soon"}]}"#;
const FROM_MAIN: &str = r#"{"tasks":[{"id":1,"reason":"from main"}]}"#;
const FROM_DECISION: &str = r#"{"tasks":[{"id":1,"reason":"from decision"}]}"#;

/// A plan reply with no recommendations in it is stored `Failed`, so it is
/// retried, and the command fails.
#[tokio::test]
async fn an_unreadable_plan_reply_fails() {
    let s = seeded();
    let mock = Arc::new(MockClient::new("do the thing"));
    let err = commands::report_command(
        &s,
        ReportKind::Plan,
        None,
        false,
        &now(wednesday()),
        &|_| Ok(handle(&mock)),
        &mut Vec::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, 1);
    let report = s
        .latest_report(ReportKind::Plan, date(2026, 11, 16))
        .unwrap()
        .unwrap();
    assert_eq!(report.status, ReportStatus::Failed);
    assert!(s.ready_report_keys().unwrap().is_empty());
}

/// `handle(main)` with `decision` as the decision model on `decision.test`.
fn with_decision(main: &Arc<MockClient>, decision: &Arc<MockClient>) -> LlmHandle {
    LlmHandle {
        decision: Some(Endpoint {
            client: decision.clone(),
            host: Some("decision.test".into()),
        }),
        ..handle(main)
    }
}

#[tokio::test]
async fn plan_uses_the_decision_model_and_summary_does_not() {
    let s = seeded();
    let main = Arc::new(MockClient::new(FROM_MAIN));
    let decision = Arc::new(MockClient::new(FROM_DECISION));
    let mut out = Vec::new();
    let make = |_| Ok(with_decision(&main, &decision));
    commands::report_command(
        &s,
        ReportKind::Plan,
        None,
        false,
        &now(wednesday()),
        &make,
        &mut out,
    )
    .await
    .unwrap();
    let plan = s
        .latest_report(ReportKind::Plan, date(2026, 11, 16))
        .unwrap()
        .unwrap();
    assert_eq!(plan.content_md.as_deref(), Some(FROM_DECISION));
    assert_eq!(plan.endpoint_host.as_deref(), Some("decision.test"));
    assert!(main.requests.lock().unwrap().is_empty());

    commands::report_command(
        &s,
        ReportKind::Summary,
        Some("this"),
        false,
        &now(wednesday()),
        &make,
        &mut Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(main.requests.lock().unwrap().len(), 1);
    assert_eq!(decision.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn plan_falls_back_to_the_main_endpoint() {
    let s = seeded();
    let main = Arc::new(MockClient::new(FROM_MAIN));
    let decision = Arc::new(MockClient::new("unused"));
    decision.push_reply(Err(LlmError::Connection("connection refused".into())));
    commands::report_command(
        &s,
        ReportKind::Plan,
        None,
        false,
        &now(wednesday()),
        &|_| Ok(with_decision(&main, &decision)),
        &mut Vec::new(),
    )
    .await
    .unwrap();
    let plan = s
        .latest_report(ReportKind::Plan, date(2026, 11, 16))
        .unwrap()
        .unwrap();
    assert_eq!(plan.status, ReportStatus::Ready);
    assert_eq!(plan.content_md.as_deref(), Some(FROM_MAIN));
    assert_eq!(plan.endpoint_host.as_deref(), Some("mock.test"));
}

#[tokio::test]
async fn failure_is_stored_as_failed_and_returns_an_error() {
    let s = seeded();
    let mock = Arc::new(MockClient::new("unused"));
    mock.push_reply(Err(LlmError::RateLimited));
    let mut out = Vec::new();
    let err = commands::report_command(
        &s,
        ReportKind::Summary,
        Some("this"),
        false,
        &now(wednesday()),
        &|_| Ok(handle(&mock)),
        &mut out,
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, 1);
    assert!(err.message.contains("rate limited"), "{}", err.message);
    assert!(out.is_empty());
    let report = s
        .latest_report(ReportKind::Summary, date(2026, 11, 16))
        .unwrap()
        .unwrap();
    assert_eq!(report.status, ReportStatus::Failed);
    assert!(report.error.unwrap().contains("rate limited"));
    assert!(report.content_md.is_none());
    // A failed report does not count as made.
    assert!(s.ready_report_keys().unwrap().is_empty());
}

#[tokio::test]
async fn facts_never_call_the_client() {
    let s = seeded();
    for kind in [ReportKind::Summary, ReportKind::Plan] {
        let mut out = Vec::new();
        commands::report_command(
            &s,
            kind,
            Some("this"),
            true,
            &now(wednesday()),
            &|_| panic!("--facts must not build a client"),
            &mut out,
        )
        .await
        .unwrap();
        let out = text(out);
        assert!(out.contains("ship the thing"), "{out}");
        assert!(out.ends_with('\n'));
    }
    assert!(
        s.list_reports(ReportKind::Plan, date(2026, 11, 16))
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn unconfigured_llm_is_exit_2_and_stores_nothing() {
    let s = seeded();
    let err = commands::report_command(
        &s,
        ReportKind::Plan,
        None,
        false,
        &now(wednesday()),
        &|_| -> CliResult<LlmHandle> {
            fortnight_cli::llm_setup::build_client(&Default::default(), None)
        },
        &mut Vec::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, 2);
    assert!(s.all_reports().unwrap().is_empty());
}

// -------------------------------------------------------------- run-due

#[tokio::test]
async fn run_due_generates_what_is_due_once() {
    let s = seeded();
    let mock = Arc::new(MockClient::new(PLAN_REPLY));
    // End of Friday 20th: the summary for the week of the 16th is due.
    // Recommendations never are: the app makes them when Today shows them.
    let when = now(date(2026, 11, 20));
    let mut out = Vec::new();
    commands::run_due(&s, &when, &|_| Ok(handle(&mock)), &mut out)
        .await
        .unwrap();
    let out = text(out);
    assert_eq!(out.lines().count(), 1, "{out}");
    assert!(out.contains("summary 2026-11-16: ready"));
    assert_eq!(s.ready_report_keys().unwrap().len(), 1);
    assert_eq!(mock.requests.lock().unwrap().len(), 1);

    // Nothing more to do: succeeds without building a client.
    let mut out = Vec::new();
    commands::run_due(&s, &when, &|_| panic!("nothing is due"), &mut out)
        .await
        .unwrap();
    assert_eq!(text(out), "Nothing is due.\n");
}

#[tokio::test]
async fn run_due_fails_non_zero_when_a_report_fails() {
    let s = seeded();
    let mock = Arc::new(MockClient::new(PLAN_REPLY));
    mock.push_reply(Err(LlmError::Timeout));
    let mut out = Vec::new();
    let err = commands::run_due(
        &s,
        &now(date(2026, 11, 20)),
        &|_| Ok(handle(&mock)),
        &mut out,
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, 1);
    assert_eq!(err.message, "1 of 1 reports failed");
    let out = text(out);
    assert_eq!(out.lines().count(), 1, "{out}");
    assert!(out.contains("summary 2026-11-16: FAILED"));
    // It stays due.
    assert!(s.ready_report_keys().unwrap().is_empty());
    let failed = s.all_reports().unwrap();
    assert!(failed.iter().any(|r| r.status == ReportStatus::Failed));
}

// --------------------------------------------------------------- export

#[test]
fn export_to_stdout_and_file_in_both_formats() {
    let s = seeded();
    for format in [ExportFormat::Markdown, ExportFormat::Json] {
        let tasks = s.list_tasks().unwrap();
        let events = s.events_between(Timestamp::MIN, Timestamp::MAX).unwrap();
        let reports = s.all_reports().unwrap();
        let expected = export::export_all(
            format,
            &tasks,
            &events,
            &reports,
            wednesday(),
            &TimeZone::UTC,
        );

        let mut out = Vec::new();
        commands::export(&s, format, &now(wednesday()), None, &mut out).unwrap();
        let out = text(out);
        assert_eq!(out.trim_end(), expected.trim_end());
        if format == ExportFormat::Json {
            serde_json::from_str::<serde_json::Value>(&out).expect("valid JSON");
        }

        let path = std::env::temp_dir().join(format!(
            "fortnight-cli-export-{}-{}.{}",
            std::process::id(),
            format.extension(),
            format.extension()
        ));
        let mut out = Vec::new();
        commands::export(&s, format, &now(wednesday()), Some(&path), &mut out).unwrap();
        assert!(out.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        std::fs::remove_file(&path).unwrap();
    }
    let err = commands::export(
        &s,
        ExportFormat::Json,
        &now(wednesday()),
        Some(std::path::Path::new("/nonexistent-dir/x/out.json")),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code, 1);
}

// ------------------------------------------------------------ lock warning

#[test]
fn lock_warning_only_for_other_hosts() {
    let dir = std::env::temp_dir().join(format!("fortnight-cli-lock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(lock_warning(&dir, "anywhere"), None);

    let (_lock, previous) = datadir::acquire_lock(&dir).unwrap();
    assert!(previous.is_none());
    let this = datadir::host_name();
    assert_eq!(lock_warning(&dir, &this), None);
    let warning = lock_warning(&dir, "some-other-machine").expect("warns");
    assert!(warning.contains(&this), "{warning}");
    assert!(warning.starts_with("warning:"));
    drop(_lock);
    assert_eq!(lock_warning(&dir, "some-other-machine"), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lock_warning_ignores_local_suffix_on_the_same_mac() {
    let dir = std::env::temp_dir().join(format!("fortnight-cli-lock-local-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let lock = serde_json::json!({
        "host": "Foo.local",
        "pid": std::process::id(),
        "since": "2026-11-16T09:00:00Z",
    });
    std::fs::write(datadir::lock_path(&dir), lock.to_string()).unwrap();
    assert_eq!(lock_warning(&dir, "Foo"), None);
    assert_eq!(lock_warning(&dir, "foo.local"), None);
    assert!(lock_warning(&dir, "Bar").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------- decision model, empties

#[tokio::test]
async fn only_recommendations_ask_for_the_decision_model() {
    let s = seeded();
    let mock = Arc::new(MockClient::new(PLAN_REPLY));
    let asked = std::cell::RefCell::new(Vec::new());
    let make = |uses_decision| {
        asked.borrow_mut().push(uses_decision);
        Ok(handle(&mock))
    };
    for kind in [ReportKind::Summary, ReportKind::Plan] {
        commands::report_command(
            &s,
            kind,
            Some("this"),
            false,
            &now(wednesday()),
            &make,
            &mut Vec::new(),
        )
        .await
        .unwrap();
    }
    assert_eq!(*asked.borrow(), [false, true]);
}

#[tokio::test]
async fn run_due_does_not_ask_for_the_decision_model() {
    let s = seeded();
    s.set_status(1, TaskStatus::Done).unwrap();
    let mock = Arc::new(MockClient::new("generated"));
    let asked = std::cell::RefCell::new(Vec::new());
    let make = |uses_decision| {
        asked.borrow_mut().push(uses_decision);
        Ok(handle(&mock))
    };
    let mut out = Vec::new();
    commands::run_due(&s, &now(date(2026, 11, 20)), &make, &mut out)
        .await
        .unwrap();
    let out = text(out);
    assert_eq!(out, "summary 2026-11-16: ready (report 1)\n");
    assert_eq!(*asked.borrow(), [false]);
    assert_eq!(mock.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn empty_facts_call_nothing_and_store_nothing() {
    let s = store();
    let mut out = Vec::new();
    commands::report_command(
        &s,
        ReportKind::Summary,
        Some("this"),
        false,
        &now(wednesday()),
        &|_| panic!("nothing to summarise"),
        &mut out,
    )
    .await
    .unwrap();
    assert_eq!(
        text(out),
        "Nothing to summarise for the week of 2026-11-16.\n"
    );
    // A task planned for later is not on today's list.
    add_task(&s, "later", Some(date(2026, 11, 25)));
    let mut out = Vec::new();
    commands::report_command(
        &s,
        ReportKind::Plan,
        None,
        false,
        &now(wednesday()),
        &|_| panic!("nothing to recommend"),
        &mut out,
    )
    .await
    .unwrap();
    assert_eq!(text(out), "Nothing to recommend for today.\n");

    let mut out = Vec::new();
    commands::run_due(
        &store(),
        &now(date(2026, 11, 20)),
        &|_| panic!("nothing to generate"),
        &mut out,
    )
    .await
    .unwrap();
    assert_eq!(text(out), "summary 2026-11-16: nothing to summarise\n");
    assert!(s.all_reports().unwrap().is_empty());
}

#[tokio::test]
async fn run_due_reports_connect_errors_that_are_not_not_configured() {
    let s = seeded();
    let when = now(date(2026, 11, 20));
    let mut out = Vec::new();
    let err = commands::run_due(
        &s,
        &when,
        &|_| -> CliResult<LlmHandle> {
            Err(CliError::failure(
                "could not connect: refusing to send the API key over plain http to 10.0.0.5",
            ))
        },
        &mut out,
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, 1);
    assert!(err.message.contains("plain http"));
    assert!(!text(out).contains("No LLM endpoint"));

    // Other usage errors are not "not configured" either.
    let err = commands::run_due(
        &s,
        &when,
        &|_| -> CliResult<LlmHandle> { Err(CliError::usage("bad protocol")) },
        &mut Vec::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, 2);

    let mut out = Vec::new();
    commands::run_due(
        &s,
        &when,
        &|_| -> CliResult<LlmHandle> { Err(CliError::not_configured("no endpoint")) },
        &mut out,
    )
    .await
    .unwrap();
    assert!(text(out).contains("1 due. No LLM endpoint is configured"));
    assert!(s.all_reports().unwrap().is_empty());
}
