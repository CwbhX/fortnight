//! Priority suggestions: while a task form is open and a decision model is
//! set up, its title and notes are sent to the model once typing pauses,
//! and the suggested priority is shown on the form that asked. A new task
//! (the dialog's) takes it as its priority unless one was picked by hand.

use crate::{Message, Ready};
use fortnight_app::top_bar::Tab;
use fortnight_core::domain::{Priority, TaskId};
use iced::Task;
use std::time::Duration;

/// The priority suggestion requests.
#[derive(Default)]
pub(crate) struct Suggestions {
    /// Bumped by each edit of a new task's title or notes, so only the
    /// latest priority suggestion lands.
    seq: u64,
    /// The open task form and its title and notes when a suggestion was
    /// last asked for: a change asks again.
    form: Option<(FormKey, String, String)>,
    /// The priority suggestion in flight (it holds the key: Lock aborts it).
    request: Option<iced::task::Handle>,
}

impl Suggestions {
    /// Lock: aborts the suggestion in flight, and ignores one already on
    /// its way.
    pub(super) fn cancel(&mut self) {
        if let Some(handle) = self.request.take() {
            handle.abort();
        }
        // A suggestion already on its way is ignored.
        self.seq += 1;
    }
}

/// Which task form a priority suggestion is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormKey {
    /// The task dialog (new or existing task).
    Dialog,
    /// Today's inline form for this task.
    Inline(TaskId),
}

impl Ready {
    /// The open task form, its title and its notes: the task dialog's, or
    /// Today's inline form.
    fn open_form_text(&self) -> Option<(FormKey, String, String)> {
        if let Some(d) = &self.dialog {
            return Some((FormKey::Dialog, d.form.title.clone(), d.form.notes.text()));
        }
        if self.tab != Tab::Today {
            return None;
        }
        self.day_view
            .form_text()
            .map(|(id, title, notes)| (FormKey::Inline(id), title, notes))
    }

    /// With a decision model set up: when a task form opens or its title or
    /// notes change, ask for a suggested priority once typing pauses.
    pub(crate) fn maybe_suggest(&mut self) -> Task<Message> {
        if !self.decision_active() {
            self.suggest.cancel();
            self.suggest.form = None;
            return Task::none();
        }
        let now = self.open_form_text();
        if now == self.suggest.form {
            return Task::none();
        }
        if let Some(old) = self.suggest.request.take() {
            old.abort();
        }
        let has_title = now.as_ref().is_some_and(|(_, t, _)| !t.trim().is_empty());
        self.suggest.form = now;
        self.suggest.seq += 1;
        if !has_title {
            return Task::none();
        }
        let seq = self.suggest.seq;
        Task::perform(tokio::time::sleep(Duration::from_millis(500)), move |_| {
            Message::SuggestPriority(seq)
        })
    }

    /// Typing paused (`Message::SuggestPriority`): asks the decision model
    /// for a priority, if `seq` is still the latest edit. `None` when there
    /// is nothing to ask (the message ends there).
    pub(crate) fn suggest_priority(&mut self, seq: u64) -> Option<Task<Message>> {
        let (form, title, notes) = self.suggest.form.clone()?;
        if seq != self.suggest.seq || title.trim().is_empty() {
            return None;
        }
        // A private task's title and notes never leave the machine.
        let task = match form {
            FormKey::Dialog => self.dialog.as_ref().and_then(|d| d.task),
            FormKey::Inline(id) => Some(id),
        };
        if task.and_then(|id| self.task(id)).is_some_and(|t| t.private) {
            return None;
        }
        let client = self.decision_model()?;
        let (task, handle) = Task::perform(
            async move { client.suggest_priority(&title, &notes).await.ok() },
            move |p| Message::PrioritySuggested(seq, p),
        )
        .abortable();
        if let Some(old) = self.suggest.request.replace(handle) {
            old.abort();
        }
        Some(task)
    }

    /// A suggested priority came back: shown on the form that asked, if
    /// `seq` is still the latest edit.
    pub(crate) fn priority_suggested(&mut self, seq: u64, priority: Option<Priority>) {
        if seq == self.suggest.seq {
            self.suggest.request = None;
            match self.suggest.form.as_ref().map(|(form, _, _)| *form) {
                Some(FormKey::Dialog) => {
                    if let Some(dialog) = self.dialog.as_mut() {
                        let new = dialog.task.is_none();
                        dialog.form.suggest(priority, new);
                    }
                }
                Some(FormKey::Inline(id)) => self.day_view.set_suggested(id, priority),
                None => {}
            }
        }
    }
}
