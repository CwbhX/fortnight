//! The Tasks view: quick-add bar and sections. Owner: agent E.

use fortnight_core::domain::{NewTask, Task, TaskId};
use iced::Element;
use iced::widget::text;
use jiff::civil::Date;

#[derive(Debug, Clone)]
pub enum Message {
    QuickAddChanged(String),
    QuickAddSubmitted,
}

/// What `main.rs` must do. Contract: do not change without the orchestrator.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Add(NewTask),
    SetDone(TaskId, bool),
    /// Save the editable fields (title, notes, project, due_on).
    Update(Task),
    Delete(TaskId),
}

#[derive(Debug, Default)]
pub struct TasksView {
    quick_add: String,
}

impl TasksView {
    pub fn new() -> Self {
        Self::default()
    }

    /// A task that focuses the quick-add input (called at launch).
    pub fn focus_quick_add() -> iced::Task<Message> {
        iced::Task::none()
    }

    /// `tasks` is every task (done or not); `today` is the local date.
    pub fn update(&mut self, message: Message, _tasks: &[Task], _today: Date) -> Action {
        match message {
            Message::QuickAddChanged(s) => {
                self.quick_add = s;
                Action::None
            }
            Message::QuickAddSubmitted => Action::None,
        }
    }

    pub fn view<'a>(&'a self, _tasks: &'a [Task], _today: Date) -> Element<'a, Message> {
        text("Tasks (agent E)").into()
    }
}
