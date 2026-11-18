//! The task dialog (the Week tab's `+`, or a click on a card): opening it
//! for a new task or an edit, and its messages (form edits, save, delete,
//! close). The dialog's state is `TaskDialog`, a field of `Ready`.

use crate::{Message, Ready, TaskDialog};
use fortnight_app::views::{day, task_form};
use fortnight_core::domain::{TaskId, TaskStatus};
use iced::Task;
use iced::widget::operation;
use jiff::civil::Date;

impl Ready {
    /// Opens the task dialog for a new task on `day` and focuses its title.
    pub(crate) fn open_new_task(&mut self, day: Date) -> Task<Message> {
        self.dialog = Some(TaskDialog {
            day: Some(day),
            form: task_form::EditForm::blank(&self.tasks, self.settings.schedule.week_start),
            task: None,
        });
        operation::focus(task_form::DIALOG_TITLE_ID)
    }

    /// Opens the task dialog to edit task `id`.
    pub(crate) fn open_edit(&mut self, id: TaskId) -> Task<Message> {
        if let Some(task) = self.task(id) {
            self.dialog = Some(TaskDialog {
                day: task.day(),
                form: task_form::open_form(task, &self.tasks, self.settings.schedule.week_start),
                task: Some(id),
            });
            return crate::unfocus();
        }
        Task::none()
    }

    /// A message from the task dialog: a form edit, `Save` (add or save,
    /// then close), `Delete` (move the task to Deleted) or `Close`.
    pub(crate) fn dialog_message(&mut self, message: day::Message) -> Task<Message> {
        let today = self.today();
        let Some(dialog) = self.dialog.as_mut() else {
            return Task::none();
        };
        if dialog.form.handle(&message, today) {
            // The dialog is drawn from its form directly.
            self.local = true;
            return Task::none();
        }
        match (message, dialog.task) {
            (day::Message::Save, None) => {
                let day = dialog.day.unwrap_or(today);
                match dialog.form.new_task(day) {
                    Ok(new) => {
                        self.dialog = None;
                        self.add_task(new);
                    }
                    Err(e) => dialog.form.error = Some(e),
                }
            }
            (day::Message::Save, Some(id)) => {
                let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned() else {
                    self.dialog = None;
                    return Task::none();
                };
                match dialog.form.apply(&task) {
                    Ok(updated) => {
                        self.dialog = None;
                        if updated != task {
                            return self.day_action(day::Action::Update(updated));
                        }
                    }
                    Err(e) => dialog.form.error = Some(e),
                }
            }
            (day::Message::Delete, Some(id)) => {
                self.dialog = None;
                self.set_status(id, TaskStatus::Dropped);
            }
            (day::Message::Close, _) => self.dialog = None,
            _ => {}
        }
        Task::none()
    }
}
