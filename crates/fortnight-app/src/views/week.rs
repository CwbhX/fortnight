//! The This-week view: open tasks grouped by day, and the Advice card.
//! Owner: agent H.

use fortnight_core::domain::{Report, Task, TaskId, WeekRef};
use iced::Element;
use iced::widget::text;
use jiff::civil::Date;

#[derive(Debug, Clone)]
pub enum Message {}

/// What `main.rs` must do. Contract: do not change without the orchestrator.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Generate (or regenerate) the advice for the current week.
    Generate,
    SetDone(TaskId, bool),
}

/// Everything the view shows, supplied by `main.rs`.
#[derive(Debug, Clone, PartialEq)]
pub struct WeekData {
    pub week: WeekRef,
    pub week_label: String,
    pub today: Date,
    /// Every task; the view picks the ones to show.
    pub tasks: Vec<Task>,
    /// The deterministic plan facts, as Markdown.
    pub facts_md: String,
    /// The latest advice for the week, if any (may be `Failed`).
    pub advice: Option<Report>,
    pub generating: bool,
    /// Why generating is not possible right now; `None` when it is.
    pub blocked: Option<String>,
}

#[derive(Debug, Default)]
pub struct WeekView {}

impl WeekView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces what the view shows. Called by `main.rs` when data changes.
    pub fn load(&mut self, _data: WeekData) {}

    pub fn update(&mut self, message: Message) -> Action {
        match message {}
    }

    pub fn view(&self) -> Element<'_, Message> {
        text("This week (agent H)").into()
    }
}
