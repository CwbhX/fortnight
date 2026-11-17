//! Export of tasks, history and reports to Markdown or JSON. Owner: agent F
//! (round 2). Pure functions: the caller gathers the data and writes the
//! file.
//!
//! Moments are UTC everywhere in the core and an export has no time zone to
//! consult, so the dates and times in the Markdown are UTC.

use crate::domain::{
    Event, MAX_PRIORITY, Report, ReportKind, ReportStatus, Task, TaskId, TaskStatus,
};
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::Serialize;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportFormat {
    Markdown,
    Json,
}

impl ExportFormat {
    /// `md` or `json`.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Markdown => "md",
            ExportFormat::Json => "json",
        }
    }

    /// Accepts `md`, `markdown` and `json`, any case.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "md" | "markdown" => Some(ExportFormat::Markdown),
            "json" => Some(ExportFormat::Json),
            _ => None,
        }
    }
}

/// Everything: every task (private ones included, marked), every event and
/// every report version. `today` labels the export ("Exported 2026-11-16").
pub fn export_all(
    format: ExportFormat,
    tasks: &[Task],
    events: &[Event],
    reports: &[Report],
    today: Date,
) -> String {
    match format {
        ExportFormat::Markdown => all_markdown(tasks, events, reports, today),
        ExportFormat::Json => all_json(tasks, events, reports, today),
    }
}

// ------------------------------------------------------------------- JSON

/// Field order here is the order in the file.
#[derive(Serialize)]
struct JsonExport<'a> {
    format: &'static str,
    version: u32,
    exported_on: Date,
    tasks: &'a [Task],
    events: &'a [Event],
    reports: &'a [Report],
}

fn all_json(tasks: &[Task], events: &[Event], reports: &[Report], today: Date) -> String {
    let doc = JsonExport {
        format: "fortnight-export",
        version: 1,
        exported_on: today,
        tasks,
        events,
        reports,
    };
    let mut out = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into());
    out.push('\n');
    out
}

// --------------------------------------------------------------- Markdown

fn all_markdown(tasks: &[Task], events: &[Event], reports: &[Report], today: Date) -> String {
    let mut out = format!("# Fortnight export, {today}\n");

    // Log lines attached to a task that is in the export go under that task;
    // the rest (free-standing, or for a task that no longer exists) are
    // listed by date afterwards.
    let task_ids: HashSet<TaskId> = tasks.iter().map(|t| t.id).collect();
    let mut by_task: HashMap<TaskId, Vec<&Event>> = HashMap::new();
    let mut loose: Vec<&Event> = Vec::new();
    for e in events.iter().filter(|e| e.kind.is_log()) {
        match e.task_id {
            Some(id) if task_ids.contains(&id) => by_task.entry(id).or_default().push(e),
            _ => loose.push(e),
        }
    }
    for list in by_task.values_mut() {
        list.sort_by_key(|e| (e.at, e.id));
    }
    loose.sort_by_key(|e| (e.at, e.id));

    out.push_str("\n## Tasks\n");
    if tasks.is_empty() {
        out.push_str("\nNo tasks.\n");
    }

    let mut open: Vec<&Task> = tasks.iter().filter(|t| t.status.is_open()).collect();
    open.sort_by(|a, b| {
        let due = match (a.due_on, b.due_on) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        due.then(b.priority.cmp(&a.priority))
            .then(a.position.total_cmp(&b.position))
            .then(a.id.cmp(&b.id))
    });
    let mut done: Vec<&Task> = tasks.iter().filter(|t| t.is_done()).collect();
    done.sort_by_key(|t| (Reverse(t.completed_at.unwrap_or(t.updated_at)), t.id));
    let mut dropped: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Dropped)
        .collect();
    dropped.sort_by_key(|t| (Reverse(t.updated_at), t.id));

    for (heading, group) in [("Open", open), ("Done", done), ("Dropped", dropped)] {
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!("\n### {heading}\n\n"));
        for task in group {
            push_task(&mut out, task, by_task.get(&task.id));
        }
    }

    if !loose.is_empty() {
        out.push_str("\n## Log\n");
        let mut current: Option<Date> = None;
        for e in loose {
            let day = utc_date(e.at);
            if current != Some(day) {
                out.push_str(&format!("\n### {day}\n\n"));
                current = Some(day);
            }
            let text = e.log_text().unwrap_or_default();
            out.push_str(&format!(
                "- {} · {}\n",
                utc_time(e.at),
                indent_rest(text, 2)
            ));
        }
    }

    out.push_str("\n## Reports\n");
    if reports.is_empty() {
        out.push_str("\nNo reports.\n");
    }
    // One entry per (kind, week): its newest version, plus a count of the
    // older ones. Newest week first; within a week, Summary before Plan.
    let mut groups: BTreeMap<(Reverse<Date>, u8), Vec<&Report>> = BTreeMap::new();
    for r in reports {
        let kind_order = match r.kind {
            ReportKind::Summary => 0,
            ReportKind::Plan => 1,
        };
        groups
            .entry((Reverse(r.week_start), kind_order))
            .or_default()
            .push(r);
    }
    for versions in groups.values() {
        let Some(latest) = versions.iter().max_by_key(|r| (r.created_at, r.id)) else {
            continue;
        };
        out.push_str(&format!("\n### {}\n\n", report_heading(latest)));
        let earlier = versions.len() - 1;
        let mut notes: Vec<String> = Vec::new();
        if earlier > 0 {
            notes.push(format!(
                "{earlier} earlier version{}",
                if earlier == 1 { "" } else { "s" }
            ));
        }
        if latest.status == ReportStatus::Failed {
            notes.push("failed".into());
        }
        if latest.edited_md.is_some() {
            notes.push("edited".into());
        }
        if let Some(model) = &latest.model {
            notes.push(format!("model {model}"));
        }
        if !notes.is_empty() {
            out.push_str(&format!("_{}_\n\n", notes.join(" · ")));
        }
        push_report_body(&mut out, latest);
    }
    out
}

fn push_task(out: &mut String, task: &Task, logs: Option<&Vec<&Event>>) {
    let mark = match task.status {
        TaskStatus::Todo => "[ ]",
        TaskStatus::Doing => "[~]",
        TaskStatus::Done => "[x]",
        TaskStatus::Dropped => "[-]",
    };
    let mut line = format!("- {mark} **{}**", task.title);
    if task.private {
        line.push_str(" (private)");
    }
    let mut parts: Vec<String> = Vec::new();
    if task.status == TaskStatus::Doing {
        parts.push("in progress".into());
    }
    if let Some(p) = &task.project {
        parts.push(format!("project: {p}"));
    }
    if let Some(d) = task.due_on {
        parts.push(format!("due: {d}"));
    }
    if let Some(d) = task.planned_for {
        parts.push(format!("planned: {d}"));
    }
    if let Some(m) = task.estimate_minutes {
        parts.push(format!("estimate: {}", format_minutes(m)));
    }
    if task.priority > 0 {
        parts.push(format!(
            "priority: {}",
            "!".repeat(usize::from(task.priority.min(MAX_PRIORITY)))
        ));
    }
    if let (true, Some(at)) = (task.is_done(), task.completed_at) {
        parts.push(format!("completed: {}", utc_date(at)));
    }
    if !parts.is_empty() {
        line.push_str(" · ");
        line.push_str(&parts.join(" · "));
    }
    out.push_str(&line);
    out.push('\n');

    let notes = task.notes.trim();
    if !notes.is_empty() {
        for l in notes.lines() {
            if l.trim().is_empty() {
                out.push('\n');
            } else {
                out.push_str(&format!("  {l}\n"));
            }
        }
    }
    for e in logs.into_iter().flatten() {
        let text = e.log_text().unwrap_or_default();
        out.push_str(&format!(
            "  - {} {} · {}\n",
            utc_date(e.at),
            utc_time(e.at),
            indent_rest(text, 4)
        ));
    }
}

/// "Summary for the week of 2026-11-16".
fn report_heading(report: &Report) -> String {
    let kind = match report.kind {
        ReportKind::Summary => "Summary",
        ReportKind::Plan => "Advice",
    };
    format!("{kind} for the week of {}", report.week_start)
}

/// The report's text (edit preferred), or its error if it failed.
fn push_report_body(out: &mut String, report: &Report) {
    let text = report.text().filter(|t| !t.trim().is_empty());
    match text {
        Some(text) => {
            out.push_str(text.trim_end());
            out.push('\n');
        }
        None if report.status != ReportStatus::Failed => out.push_str("_No text._\n"),
        None => {}
    }
    if report.status == ReportStatus::Failed {
        if text.is_some() {
            out.push('\n');
        }
        let error = report.error.as_deref().unwrap_or("no error recorded");
        out.push_str(&format!("**Failed:** {error}\n"));
    }
}

/// 120 → "~2h", 90 → "~1h30m", 30 → "~30m".
fn format_minutes(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("~{m}m"),
        (h, 0) => format!("~{h}h"),
        (h, m) => format!("~{h}h{m}m"),
    }
}

/// Indent every line after the first, so multi-line text stays inside its
/// list item.
fn indent_rest(text: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    text.trim_end().replace('\n', &format!("\n{pad}"))
}

fn utc_date(at: Timestamp) -> Date {
    TimeZone::UTC.to_datetime(at).date()
}

fn utc_time(at: Timestamp) -> String {
    let t = TimeZone::UTC.to_datetime(at).time();
    format!("{:02}:{:02}", t.hour(), t.minute())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::EventKind;
    use jiff::civil::date;
    use serde::Deserialize;

    const TODAY: Date = date(2026, 11, 16);

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn task(id: TaskId, title: &str) -> Task {
        Task {
            id,
            title: title.into(),
            position: id as f64,
            created_at: ts("2026-11-10T09:00:00Z"),
            updated_at: ts("2026-11-10T09:00:00Z"),
            ..Task::default()
        }
    }

    fn log(id: i64, task_id: Option<TaskId>, at: &str, text: &str) -> Event {
        Event {
            id,
            task_id,
            at: ts(at),
            kind: EventKind::Log { text: text.into() },
        }
    }

    fn report(id: i64, kind: ReportKind, week: Date, created: &str, text: &str) -> Report {
        Report {
            id,
            kind,
            week_start: week,
            status: ReportStatus::Ready,
            digest_json: "{}".into(),
            content_md: Some(text.into()),
            edited_md: None,
            model: None,
            endpoint_host: None,
            prompt_version: 1,
            error: None,
            created_at: ts(created),
        }
    }

    fn sample() -> (Vec<Task>, Vec<Event>, Vec<Report>) {
        let mut a = task(1, "Write spec");
        a.project = Some("Atlas".into());
        a.due_on = Some(date(2026, 11, 20));
        a.estimate_minutes = Some(90);
        a.priority = 2;
        a.notes = "first line\nsecond line".into();
        let mut b = task(2, "Call the bank");
        b.private = true;
        let mut c = task(3, "Earlier deadline");
        c.due_on = Some(date(2026, 11, 18));
        let mut d = task(4, "Shipped it");
        d.status = TaskStatus::Done;
        d.completed_at = Some(ts("2026-11-12T10:00:00Z"));
        let mut e = task(5, "Never mind");
        e.status = TaskStatus::Dropped;
        let events = vec![
            log(1, Some(1), "2026-11-12T14:30:00Z", "Drafted the outline"),
            log(2, None, "2026-11-13T09:05:00Z", "Team offsite\nsecond line"),
            Event {
                id: 3,
                task_id: Some(1),
                at: ts("2026-11-12T08:00:00Z"),
                kind: EventKind::Created {
                    title: "Write spec".into(),
                },
            },
        ];
        let mut failed = report(
            3,
            ReportKind::Plan,
            date(2026, 11, 16),
            "2026-11-16T08:00:00Z",
            "",
        );
        failed.status = ReportStatus::Failed;
        failed.content_md = None;
        failed.error = Some("endpoint returned 500".into());
        let mut edited = report(
            2,
            ReportKind::Summary,
            date(2026, 11, 9),
            "2026-11-14T16:00:00Z",
            "model text v2",
        );
        edited.edited_md = Some("my edited text".into());
        let reports = vec![
            report(
                1,
                ReportKind::Summary,
                date(2026, 11, 9),
                "2026-11-13T16:00:00Z",
                "model text v1",
            ),
            edited,
            failed,
        ];
        (vec![a, b, c, d, e], events, reports)
    }

    fn pos(haystack: &str, needle: &str) -> usize {
        haystack
            .find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in:\n{haystack}"))
    }

    #[test]
    fn json_round_trips_through_the_domain_types() {
        #[derive(Deserialize)]
        struct Back {
            format: String,
            version: u32,
            exported_on: Date,
            tasks: Vec<Task>,
            events: Vec<Event>,
            reports: Vec<Report>,
        }
        let (tasks, events, reports) = sample();
        let json = export_all(ExportFormat::Json, &tasks, &events, &reports, TODAY);
        assert!(json.contains("\n  \"tasks\""), "pretty printed");
        assert!(pos(&json, "\"format\"") < pos(&json, "\"tasks\""));
        let back: Back = serde_json::from_str(&json).unwrap();
        assert_eq!(back.format, "fortnight-export");
        assert_eq!(back.version, 1);
        assert_eq!(back.exported_on, TODAY);
        assert_eq!(back.tasks, tasks);
        assert_eq!(back.events, events);
        assert_eq!(back.reports, reports);
    }

    #[test]
    fn markdown_has_sections_in_order() {
        let (tasks, events, reports) = sample();
        let md = export_all(ExportFormat::Markdown, &tasks, &events, &reports, TODAY);
        assert!(md.starts_with("# Fortnight export, 2026-11-16\n"));
        let order = [
            "## Tasks",
            "### Open",
            "### Done",
            "### Dropped",
            "## Log",
            "## Reports",
        ];
        let mut last = 0;
        for h in order {
            let p = pos(&md, h);
            assert!(p >= last, "{h} out of order");
            last = p;
        }
    }

    #[test]
    fn open_tasks_sort_by_due_date_undated_last() {
        let (tasks, events, reports) = sample();
        let md = export_all(ExportFormat::Markdown, &tasks, &events, &reports, TODAY);
        let earlier = pos(&md, "Earlier deadline"); // due 18th
        let spec = pos(&md, "Write spec"); // due 20th
        let bank = pos(&md, "Call the bank"); // undated
        assert!(earlier < spec && spec < bank);
        assert!(pos(&md, "Shipped it") > bank);
        assert!(pos(&md, "Never mind") > pos(&md, "Shipped it"));
    }

    #[test]
    fn task_line_has_all_fields_and_private_marker() {
        let (tasks, events, reports) = sample();
        let md = export_all(ExportFormat::Markdown, &tasks, &events, &reports, TODAY);
        assert!(md.contains(
            "- [ ] **Write spec** · project: Atlas · due: 2026-11-20 · estimate: ~1h30m · priority: !!\n"
        ));
        assert!(md.contains("- [ ] **Call the bank** (private)\n"));
        assert_eq!(md.matches("(private)").count(), 1);
        assert!(md.contains("- [x] **Shipped it**"));
        assert!(md.contains("- [-] **Never mind**"));
    }

    #[test]
    fn notes_and_task_logs_sit_under_their_task() {
        let (tasks, events, reports) = sample();
        let md = export_all(ExportFormat::Markdown, &tasks, &events, &reports, TODAY);
        let spec = pos(&md, "**Write spec**");
        let notes = pos(&md, "  first line\n  second line\n");
        let logline = pos(&md, "  - 2026-11-12 14:30 · Drafted the outline");
        let next = pos(&md, "**Call the bank**");
        assert!(spec < notes && notes < logline && logline < next);
        // Non-log events are not exported to Markdown.
        assert!(!md.contains("created"));
    }

    #[test]
    fn free_standing_log_lines_are_listed_by_date() {
        let (tasks, events, reports) = sample();
        let md = export_all(ExportFormat::Markdown, &tasks, &events, &reports, TODAY);
        let log = &md[pos(&md, "## Log")..pos(&md, "## Reports")];
        assert!(log.contains("### 2026-11-13\n\n- 09:05 · Team offsite\n  second line\n"));
        assert!(!log.contains("Drafted the outline"));
    }

    #[test]
    fn reports_newest_week_first_latest_version_edit_preferred() {
        let (tasks, events, reports) = sample();
        let md = export_all(ExportFormat::Markdown, &tasks, &events, &reports, TODAY);
        let reports_md = &md[pos(&md, "## Reports")..];
        let newer = pos(reports_md, "### Advice for the week of 2026-11-16");
        let older = pos(reports_md, "### Summary for the week of 2026-11-09");
        assert!(newer < older);
        assert!(reports_md.contains("my edited text"));
        assert!(!reports_md.contains("model text v2"));
        assert!(!reports_md.contains("model text v1"));
        assert!(reports_md.contains("1 earlier version"));
        assert!(reports_md.contains("**Failed:** endpoint returned 500"));
    }

    #[test]
    fn empty_inputs() {
        let md = export_all(ExportFormat::Markdown, &[], &[], &[], TODAY);
        assert!(md.contains("# Fortnight export, 2026-11-16"));
        assert!(md.contains("No tasks."));
        assert!(md.contains("No reports."));
        assert!(!md.contains("## Log"));
        let json = export_all(ExportFormat::Json, &[], &[], &[], TODAY);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["tasks"], serde_json::json!([]));
        assert_eq!(v["events"], serde_json::json!([]));
        assert_eq!(v["reports"], serde_json::json!([]));
        assert_eq!(v["exported_on"], "2026-11-16");
    }

    #[test]
    fn formats_parse() {
        assert_eq!(
            ExportFormat::parse(" Markdown "),
            Some(ExportFormat::Markdown)
        );
        assert_eq!(ExportFormat::parse("JSON"), Some(ExportFormat::Json));
        assert_eq!(ExportFormat::parse("csv"), None);
        assert_eq!(ExportFormat::Json.extension(), "json");
    }
}
