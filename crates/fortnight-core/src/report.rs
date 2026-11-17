//! Week facts, the two prompts, and `generate`. Owner: agent D.
//!
//! Flow: tasks → facts (pure, deterministic) → prompt → client → NewReport.
//! The app stores the NewReport; this module never touches the store, so
//! `generate` is a `Send` future the app can run as an async task.
//! The LLM never does arithmetic: days until due, weekdays and overdue
//! flags are computed here and handed over as facts. PLAN.md 7.3 and 7.4.

use crate::domain::{NewReport, ReportKind, Settings, Task, WeekRef};
use crate::llm::{ChatRequest, LlmClient};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::Serialize;
use std::sync::Arc;

/// Bump when a prompt changes.
pub const PROMPT_VERSION: i64 = 1;

/// Facts for the weekly summary of `week` (the week behind you).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SummaryFacts {
    pub week_start: Date,
    pub week_end: Date,
}

/// Facts for advice on `week` (the week ahead), as seen on `today`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanFacts {
    pub today: Date,
    pub week_start: Date,
    pub week_end: Date,
}

/// Builds summary facts. `tz` converts timestamps (completed_at,
/// created_at) to local civil dates.
pub fn summary_facts(_tasks: &[Task], _week: WeekRef, _tz: &TimeZone) -> SummaryFacts {
    todo!("agent D")
}

/// Builds plan facts from open tasks, relative to `today`.
pub fn plan_facts(
    _tasks: &[Task],
    _week: WeekRef,
    _today: Date,
    _settings: &Settings,
) -> PlanFacts {
    todo!("agent D")
}

/// Facts as Markdown, for the "facts" panel beside a report.
pub fn summary_facts_markdown(_facts: &SummaryFacts) -> String {
    todo!("agent D")
}

pub fn plan_facts_markdown(_facts: &PlanFacts) -> String {
    todo!("agent D")
}

/// Everything needed to call the model and store the result.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub kind: ReportKind,
    pub week: WeekRef,
    /// The facts as JSON; stored as `Report::digest_json`.
    pub digest_json: String,
    pub request: ChatRequest,
}

pub fn prepare_summary(
    _tasks: &[Task],
    _week: WeekRef,
    _tz: &TimeZone,
    _settings: &Settings,
) -> Prepared {
    todo!("agent D")
}

pub fn prepare_plan(
    _tasks: &[Task],
    _week: WeekRef,
    _today: Date,
    _settings: &Settings,
) -> Prepared {
    todo!("agent D")
}

/// Calls the model and returns a report row to insert: `Ready` with the
/// content on success, `Failed` with the error message otherwise. Never
/// panics on a client error.
pub async fn generate(
    _client: Arc<dyn LlmClient>,
    _prepared: Prepared,
    _endpoint_host: Option<String>,
) -> NewReport {
    todo!("agent D")
}
