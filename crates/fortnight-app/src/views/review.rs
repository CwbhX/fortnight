//! The Review view: the week behind. Facts beside the summary; edit, copy,
//! regenerate; a week switcher. Owner: agent G.

use fortnight_core::domain::{Report, ReportId, WeekRef};
use iced::Element;
use iced::widget::text;

#[derive(Debug, Clone)]
pub enum Message {}

/// What `main.rs` must do. Contract: do not change without the orchestrator.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Show another week; `main.rs` responds by calling `load`.
    ShowWeek(WeekRef),
    /// Generate (or regenerate) the summary for this week.
    Generate(WeekRef),
    /// Save the user's edit of a report.
    SaveEdit(ReportId, String),
    /// Copy this text to the clipboard.
    Copy(String),
}

/// Everything the view shows, supplied by `main.rs`.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewData {
    pub week: WeekRef,
    /// e.g. "Week 46 · Nov 9–15".
    pub week_label: String,
    /// The deterministic facts, as Markdown.
    pub facts_md: String,
    /// The latest summary for the week, if any (may be `Failed`).
    pub report: Option<Report>,
    /// A summary for this week is being generated right now.
    pub generating: bool,
    /// Why generating is not possible right now, e.g. "Unlock to generate"
    /// or "Set up an endpoint in Settings". `None` when it is possible.
    pub blocked: Option<String>,
}

#[derive(Debug, Default)]
pub struct ReviewView {}

impl ReviewView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces what the view shows. `main.rs` calls this after EVERY
    /// message (and on the minute tick), so it must be cheap: if `data`
    /// equals what the view already has, return early. It must keep
    /// in-progress UI state (an open editor, typed text) unless the thing it
    /// belongs to changed (a different week or report id).
    pub fn load(&mut self, _data: ReviewData) {}

    pub fn update(&mut self, message: Message) -> Action {
        match message {}
    }

    pub fn view(&self) -> Element<'_, Message> {
        text("Review (agent G)").into()
    }
}
