//! The commands, as functions over a `&dyn Store` and a writer, so tests can
//! use an in-memory store and a mock LLM. Nothing here reads the process
//! environment, stdin or the clock.

use crate::args::ListArgs;
use crate::error::{CliError, CliResult};
use crate::llm_setup::LlmHandle;
use fortnight_core::domain::{
    NewTask, Report, ReportKind, ReportStatus, Settings, Task, TaskId, TaskStatus, WeekRef,
};
use fortnight_core::export::{self, ExportFormat};
use fortnight_core::fallback::FallbackClient;
use fortnight_core::llm::{ChatEvent, ChatResponse, LlmClient, LlmError};
use fortnight_core::parse;
use fortnight_core::report::{self, Prepared};
use fortnight_core::store::Store;
use fortnight_core::week;
use futures_util::StreamExt;
use jiff::Timestamp;
use jiff::ZonedDateTime;
use jiff::civil::{Date, Time, Weekday};
use jiff::tz::TimeZone;
use std::cmp::{Ordering, Reverse};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

// ---------------------------------------------------------------- time

/// "Now" for a run. A `--today` override means the end of that day, so that
/// whatever is due by then counts as due.
pub fn now_for(today: Option<Date>, tz: TimeZone) -> CliResult<ZonedDateTime> {
    match today {
        None => Ok(Timestamp::now().to_zoned_datetime(tz)),
        Some(d) => d
            .to_datetime(Time::constant(23, 59, 59, 0))
            .to_zoned_datetime(tz)
            .map_err(|e| CliError::failure(e.to_string())),
    }
}

// ----------------------------------------------------------------- add

/// `add`: parses the quick-add words and stores the task.
pub fn add(store: &dyn Store, words: &[String], today: Date, out: &mut dyn Write) -> CliResult<()> {
    let parsed = parse::parse_quick_add(&words.join(" "), today);
    if parsed.title.trim().is_empty() {
        return Err(CliError::usage(
            "the task needs a title (only a date, project, estimate or priority was given)",
        ));
    }
    let task = store.add_task(NewTask {
        title: parsed.title,
        project: parsed.project,
        // A quick-add date is the day you plan to do it.
        planned_for: parsed.due_on,
        estimate_minutes: parsed.estimate_minutes,
        priority: parsed.priority,
        ..NewTask::default()
    })?;
    let mut extras = Vec::new();
    if let Some(d) = task.planned_for {
        extras.push(format!("on {d}"));
    }
    if let Some(p) = &task.project {
        extras.push(format!("#{p}"));
    }
    if let Some(m) = task.estimate_minutes {
        extras.push(format!("~{}", parse::format_estimate(m)));
    }
    if task.priority > 0 {
        extras.push("!".repeat(usize::from(task.priority)));
    }
    if extras.is_empty() {
        writeln!(out, "Added {}: {}", task.id, task.title)?;
    } else {
        writeln!(
            out,
            "Added {}: {} ({})",
            task.id,
            task.title,
            extras.join(", ")
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------- list

/// The tasks `list` shows, in order: overdue first (by deadline), then by
/// day (planned day, else deadline), then undated. Within a day, higher
/// priority first, then manual order.
pub fn select_tasks(
    tasks: &[Task],
    opts: &ListArgs,
    today: Date,
    week_start: Weekday,
) -> Vec<Task> {
    let this_week = week::week_containing(today, week_start);
    let mut shown: Vec<Task> = tasks
        .iter()
        .filter(|t| opts.all || t.status.is_open())
        .filter(|t| {
            if opts.week {
                t.is_overdue(today) || t.day().is_some_and(|d| this_week.contains(d))
            } else if opts.today {
                t.is_overdue(today) || t.day() == Some(today)
            } else {
                true
            }
        })
        .cloned()
        .collect();
    let group = |t: &Task| -> (u8, Option<Date>) {
        if t.is_overdue(today) {
            (0, t.due_on)
        } else if let Some(d) = t.day() {
            (1, Some(d))
        } else {
            (2, None)
        }
    };
    shown.sort_by(|a, b| {
        group(a)
            .cmp(&group(b))
            .then_with(|| Reverse(a.priority).cmp(&Reverse(b.priority)))
            .then_with(|| {
                a.position
                    .partial_cmp(&b.position)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| a.id.cmp(&b.id))
    });
    shown
}

fn status_mark(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Todo => "[ ]",
        TaskStatus::Doing => "[~]",
        TaskStatus::Done => "[x]",
        TaskStatus::Dropped => "[-]",
    }
}

/// Aligned plain-text table: id, status mark, title, project, planned day,
/// due, estimate, priority.
pub fn format_list(tasks: &[Task]) -> String {
    if tasks.is_empty() {
        return "No tasks.\n".to_string();
    }
    let mut rows: Vec<[String; 8]> = vec![[
        "ID".into(),
        "ST".into(),
        "TITLE".into(),
        "PROJECT".into(),
        "DAY".into(),
        "DUE".into(),
        "EST".into(),
        "PRI".into(),
    ]];
    for t in tasks {
        rows.push([
            t.id.to_string(),
            status_mark(t.status).to_string(),
            t.title.clone(),
            t.project.clone().unwrap_or_default(),
            t.day().map(|d| d.to_string()).unwrap_or_default(),
            t.due_on.map(|d| d.to_string()).unwrap_or_default(),
            t.estimate_minutes
                .map(parse::format_estimate)
                .unwrap_or_default(),
            "!".repeat(usize::from(t.priority)),
        ]);
    }
    let mut widths = [0usize; 8];
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in &rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            line.push_str(cell);
            let pad = widths[i] - cell.chars().count();
            line.extend(std::iter::repeat_n(' ', pad + 2));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// `list`.
pub fn list(store: &dyn Store, opts: &ListArgs, today: Date, out: &mut dyn Write) -> CliResult<()> {
    let settings = store.load_settings()?;
    let tasks = store.list_tasks()?;
    let shown = select_tasks(&tasks, opts, today, settings.schedule.week_start);
    out.write_all(format_list(&shown).as_bytes())?;
    Ok(())
}

// ------------------------------------------------------------ done, log

/// `done`: sets each task done (or todo with `undo`). Carries on past a
/// missing id and reports them all at the end.
pub fn done(store: &dyn Store, ids: &[TaskId], undo: bool, out: &mut dyn Write) -> CliResult<()> {
    let status = if undo {
        TaskStatus::Todo
    } else {
        TaskStatus::Done
    };
    let mut failures = Vec::new();
    for &id in ids {
        let result = store
            .set_status(id, status)
            .and_then(|()| store.get_task(id));
        match result {
            Ok(Some(task)) => writeln!(
                out,
                "{} {}: {}",
                if undo { "Reopened" } else { "Done" },
                task.id,
                task.title
            )?,
            Ok(None) => failures.push(format!("task {id} not found")),
            Err(e) => failures.push(e.to_string()),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(CliError::failure(failures.join("\n")))
    }
}

/// `log`: a log line, attached to `task` if given.
pub fn log(
    store: &dyn Store,
    task: Option<TaskId>,
    words: &[String],
    out: &mut dyn Write,
) -> CliResult<()> {
    let text = words.join(" ");
    if text.trim().is_empty() {
        return Err(CliError::usage("the log line is empty"));
    }
    store.add_log(task, text.trim())?;
    match task {
        Some(id) => writeln!(out, "Logged on task {id}.")?,
        None => writeln!(out, "Logged.")?,
    }
    Ok(())
}

// -------------------------------------------------------------- reports

/// `this`, `last`, or a date (any day of the week wanted).
pub fn parse_week_arg(arg: &str, today: Date, week_start: Weekday) -> CliResult<WeekRef> {
    let this = week::week_containing(today, week_start);
    match arg.trim().to_ascii_lowercase().as_str() {
        "this" => Ok(this),
        "last" => Ok(this.prev()),
        other => other
            .parse::<Date>()
            .map(|d| week::week_containing(d, week_start))
            .map_err(|_| {
                CliError::usage(format!(
                    "'{arg}' is not a week. Use this, last or a date such as 2026-11-16."
                ))
            }),
    }
}

/// The week a report command works on. With no argument: the plan is for
/// this week; the summary is for the latest week whose summary is due (as
/// the app decides), falling back to this week.
pub fn resolve_week(
    kind: ReportKind,
    arg: Option<&str>,
    now: &ZonedDateTime,
    settings: &Settings,
) -> CliResult<WeekRef> {
    let week_start = settings.schedule.week_start;
    if let Some(arg) = arg {
        return parse_week_arg(arg, now.date(), week_start);
    }
    let this = week::week_containing(now.date(), week_start);
    if kind == ReportKind::Summary {
        let due = week::due_reports(now, &settings.schedule, &[]);
        if let Some(job) = due.iter().find(|j| j.kind == ReportKind::Summary) {
            return Ok(job.week);
        }
    }
    Ok(this)
}

fn prepare(
    store: &dyn Store,
    kind: ReportKind,
    week: WeekRef,
    now: &ZonedDateTime,
    settings: &Settings,
) -> CliResult<Prepared> {
    let tasks = store.list_tasks()?;
    let events = store.events_between(Timestamp::MIN, Timestamp::MAX)?;
    let today = now.date();
    Ok(match kind {
        ReportKind::Summary => {
            report::prepare_summary(&tasks, &events, week, today, now.time_zone(), settings)
        }
        ReportKind::Plan => report::prepare_plan(&tasks, &events, week, today, settings),
    })
}

fn facts_markdown(
    store: &dyn Store,
    kind: ReportKind,
    week: WeekRef,
    now: &ZonedDateTime,
    settings: &Settings,
) -> CliResult<String> {
    let tasks = store.list_tasks()?;
    let events = store.events_between(Timestamp::MIN, Timestamp::MAX)?;
    let today = now.date();
    Ok(match kind {
        ReportKind::Summary => report::summary_facts_markdown(&report::summary_facts(
            &tasks,
            &events,
            week,
            today,
            now.time_zone(),
        )),
        ReportKind::Plan => {
            report::plan_facts_markdown(&report::plan_facts(&tasks, &events, week, today, settings))
        }
    })
}

/// Calls the model with streaming, writes the text to `out` as it arrives,
/// and stores the result (`Ready`, or `Failed` with the error). Returns the
/// stored report; a model failure is a `Failed` report, not an `Err`.
pub async fn generate_report(
    store: &dyn Store,
    prepared: Prepared,
    llm: &LlmHandle,
    out: &mut dyn Write,
) -> CliResult<Report> {
    // Recommended tasks go to the decision model, if any, falling back to
    // the main endpoint.
    let decision = match (&llm.decision, prepared.kind) {
        (Some((client, host)), ReportKind::Plan) => Some((
            Arc::new(FallbackClient::new(client.clone(), llm.client.clone())),
            host.clone(),
        )),
        _ => None,
    };
    let client: Arc<dyn LlmClient> = match &decision {
        Some((fc, _)) => fc.clone(),
        None => llm.client.clone(),
    };
    let mut stream = client.chat_stream(prepared.request.clone());
    let mut text = String::new();
    let mut model = None;
    let mut failure = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(ChatEvent::Delta(d)) => {
                // A closed pipe must not stop the report from being stored.
                let _ = out.write_all(d.as_bytes()).and_then(|()| out.flush());
                text.push_str(&d);
            }
            Ok(ChatEvent::Done { model: m }) => {
                model = Some(m);
                break;
            }
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    drop(stream);
    if !text.is_empty() && !text.ends_with('\n') {
        let _ = out.write_all(b"\n").and_then(|()| out.flush());
    }
    let result = match (failure, model) {
        (Some(e), _) => Err(e),
        (None, Some(model)) => Ok(ChatResponse { text, model }),
        (None, None) => Err(LlmError::Truncated),
    };
    let (requested_model, host) = match &decision {
        Some((fc, decision_host)) if !fc.used_fallback() => {
            (fc.model().to_string(), decision_host.clone())
        }
        _ => (llm.client.model().to_string(), llm.endpoint_host.clone()),
    };
    let new = report::finish(prepared, result, &requested_model, host);
    Ok(store.insert_report(new)?)
}

/// `summary` and `plan`. `make_client` is only called when the model is
/// needed (never with `facts_only`).
pub async fn report_command(
    store: &dyn Store,
    kind: ReportKind,
    week_arg: Option<&str>,
    facts_only: bool,
    now: &ZonedDateTime,
    make_client: &dyn Fn() -> CliResult<LlmHandle>,
    out: &mut dyn Write,
) -> CliResult<()> {
    let settings = store.load_settings()?;
    let week = resolve_week(kind, week_arg, now, &settings)?;
    if facts_only {
        let md = facts_markdown(store, kind, week, now, &settings)?;
        out.write_all(md.as_bytes())?;
        if !md.ends_with('\n') {
            writeln!(out)?;
        }
        return Ok(());
    }
    let prepared = prepare(store, kind, week, now, &settings)?;
    let llm = make_client()?;
    // The plan arrives as JSON: print it as a list once it is complete.
    let stored = match kind {
        ReportKind::Summary => generate_report(store, prepared, &llm, out).await?,
        ReportKind::Plan => {
            let stored = generate_report(store, prepared, &llm, &mut std::io::sink()).await?;
            if let Some(text) = stored.text() {
                let md = report::recommendations_markdown(text, &store.list_tasks()?);
                out.write_all(md.as_bytes())?;
                if !md.ends_with('\n') {
                    writeln!(out)?;
                }
            }
            stored
        }
    };
    match stored.status {
        ReportStatus::Ready => Ok(()),
        ReportStatus::Failed => Err(CliError::failure(
            stored.error.unwrap_or_else(|| "the report failed".into()),
        )),
    }
}

/// `run-due`: generates whatever is due. Prints one line per report.
pub async fn run_due(
    store: &dyn Store,
    now: &ZonedDateTime,
    make_client: &dyn Fn() -> CliResult<LlmHandle>,
    out: &mut dyn Write,
) -> CliResult<()> {
    let settings = store.load_settings()?;
    let existing = store.ready_report_keys()?;
    let jobs = week::due_reports(now, &settings.schedule, &existing);
    if jobs.is_empty() {
        writeln!(out, "Nothing is due.")?;
        return Ok(());
    }
    let llm = match make_client() {
        Ok(llm) => llm,
        // PLAN.md 8: with no LLM configured nothing fails; the facts are
        // there in the app and through `summary --facts`.
        Err(e) if e.code == crate::error::EXIT_USAGE => {
            writeln!(
                out,
                "{} due. No LLM endpoint is configured, so nothing was generated.",
                jobs.len()
            )?;
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let mut failed = 0;
    for job in &jobs {
        // The app may have generated it meanwhile.
        let key = fortnight_core::domain::ReportKey {
            kind: job.kind,
            week_start: job.week.start,
        };
        if store.ready_report_keys()?.contains(&key) {
            writeln!(
                out,
                "{} {}: already ready",
                job.kind.as_str(),
                job.week.start
            )?;
            continue;
        }
        let prepared = prepare(store, job.kind, job.week, now, &settings)?;
        let stored = generate_report(store, prepared, &llm, &mut std::io::sink()).await?;
        match stored.status {
            ReportStatus::Ready => writeln!(
                out,
                "{} {}: ready (report {})",
                job.kind.as_str(),
                job.week.start,
                stored.id
            )?,
            ReportStatus::Failed => {
                failed += 1;
                writeln!(
                    out,
                    "{} {}: FAILED: {}",
                    job.kind.as_str(),
                    job.week.start,
                    stored.error.as_deref().unwrap_or("unknown error")
                )?;
            }
        }
    }
    if failed == 0 {
        Ok(())
    } else {
        Err(CliError::failure(format!(
            "{failed} of {} reports failed",
            jobs.len()
        )))
    }
}

// --------------------------------------------------------------- export

/// `export`: everything, to `dest` or to `out`.
pub fn export(
    store: &dyn Store,
    format: ExportFormat,
    today: Date,
    dest: Option<&Path>,
    out: &mut dyn Write,
) -> CliResult<()> {
    let tasks = store.list_tasks()?;
    let events = store.events_between(Timestamp::MIN, Timestamp::MAX)?;
    let reports = store.all_reports()?;
    let text = export::export_all(format, &tasks, &events, &reports, today);
    match dest {
        Some(path) => std::fs::write(path, text)
            .map_err(|e| CliError::failure(format!("cannot write {}: {e}", path.display()))),
        None => {
            out.write_all(text.as_bytes())?;
            if !text.ends_with('\n') {
                writeln!(out)?;
            }
            Ok(())
        }
    }
}
