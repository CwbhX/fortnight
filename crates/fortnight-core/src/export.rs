//! Export of tasks, history and reports to Markdown or JSON. Owner: agent F
//! (round 2). Pure functions: the caller gathers the data and writes the
//! file. STUB until agent F implements it.

use crate::domain::{Event, Report, Task};
use jiff::civil::Date;

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
    let _ = (tasks, events, reports, today);
    match format {
        ExportFormat::Markdown => String::new(),
        ExportFormat::Json => "{}".into(),
    }
}

/// One report version: Markdown is its text (the edit if any) under a
/// heading naming the kind and week; JSON is the whole `Report`.
pub fn export_report(format: ExportFormat, report: &Report) -> String {
    match format {
        ExportFormat::Markdown => report.text().unwrap_or_default().to_string(),
        ExportFormat::Json => "{}".into(),
    }
}

/// A file name for `export_report`, e.g. `fortnight-summary-2026-11-16.md`.
pub fn report_file_name(format: ExportFormat, report: &Report) -> String {
    format!(
        "fortnight-{}-{}.{}",
        report.kind.as_str(),
        report.week_start,
        format.extension()
    )
}

/// A file name for `export_all`, e.g. `fortnight-export-2026-11-16.json`.
pub fn all_file_name(format: ExportFormat, today: Date) -> String {
    format!("fortnight-export-{today}.{}", format.extension())
}
