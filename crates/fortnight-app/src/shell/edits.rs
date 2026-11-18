//! Task writes and undo: what the Today and Week tabs' actions and the
//! task dialog write to the store (add, status, edit, move, reorder,
//! delete, a day's notes), each remembered on the undo stack, and undo
//! itself (Ctrl/Cmd+Z).

use crate::{Message, Ready};
use fortnight_app::views::day;
use fortnight_core::domain::{DeletedTask, EventId, Task as CoreTask, TaskId, TaskStatus};
use fortnight_core::store::Store;
use iced::Task;
use jiff::Timestamp;
use jiff::civil::Date;

/// One change that Ctrl/Cmd+Z can take back.
#[derive(Debug, Clone)]
pub(super) enum Undo {
    /// A task was added: delete it (only while it has no other history).
    Added(TaskId),
    /// A task's status or fields changed. Undo puts the task back exactly as
    /// it was (including `completed_at`) and drops the events the change
    /// wrote (ids above `last_event`), keeping log lines.
    Changed {
        before: Box<CoreTask>,
        last_event: EventId,
        /// Keep the task's position as it is at undo time (a later drag
        /// stays), unless the change was a move, which set the position.
        keep_position: bool,
    },
    /// Tasks were deleted for good (one, or all of Deleted at once); put
    /// them back with their history.
    Deleted(Vec<DeletedTask>),
}

/// How many changes undo remembers.
const UNDO_DEPTH: usize = 30;

/// The changes undo can take back, oldest first, at most `UNDO_DEPTH`.
#[derive(Default)]
pub(crate) struct UndoStack(Vec<Undo>);

impl UndoStack {
    /// Remembers a change for undo.
    fn remember(&mut self, undo: Undo) {
        self.0.push(undo);
        if self.0.len() > UNDO_DEPTH {
            self.0.remove(0);
        }
    }

    fn pop(&mut self) -> Option<Undo> {
        self.0.pop()
    }

    /// Puts back an entry whose undo failed, for another try.
    fn put_back(&mut self, undo: Undo) {
        self.0.push(undo);
    }
}

/// What undo of an edit writes back: `before`, but with the task's
/// `current` position when `keep_position` (a later drag stays).
fn reverted(before: &CoreTask, current: Option<&CoreTask>, keep_position: bool) -> CoreTask {
    let mut task = before.clone();
    if keep_position && let Some(current) = current {
        task.position = current.position;
    }
    task
}

impl Ready {
    /// Takes back the last change (Ctrl/Cmd+Z).
    pub(super) fn undo(&mut self) {
        let Some(last) = self.undo.pop() else {
            return;
        };
        let result = match &last {
            Undo::Added(id) => self.undo_add(*id),
            Undo::Changed {
                before,
                last_event,
                keep_position,
            } => self.undo_change(before, *last_event, *keep_position),
            Undo::Deleted(deleted) => {
                // Put back what can be; keep the rest for another try.
                let mut left = Vec::new();
                let mut failed = None;
                for d in deleted {
                    if failed.is_none()
                        && let Err(e) = self.store.restore_task(d)
                    {
                        failed = Some(e.to_string());
                    }
                    if failed.is_some() {
                        left.push(d.clone());
                    }
                }
                match failed {
                    None => Ok(()),
                    Some(e) => {
                        self.undo.put_back(Undo::Deleted(left));
                        self.error = Some(format!("Could not undo: {e}"));
                        self.reload_tasks();
                        return;
                    }
                }
            }
        };
        if let Err(e) = result {
            self.error = Some(format!("Could not undo: {e}"));
            // Keep the entry (and its saved copy) for another try.
            if !matches!(last, Undo::Added(_)) {
                self.undo.put_back(last);
            }
        }
        self.reload_tasks();
    }

    /// Deletes a task just added, unless it has since gained history (a log
    /// line, an edit), which deleting would lose. One transaction.
    fn undo_add(&mut self, id: TaskId) -> Result<(), String> {
        match self.store.delete_new_task(id) {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err("the task has changed since you added it. Delete it instead".into()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Puts the task back as `before` (keeping its current position unless
    /// `keep_position` is false) and drops the events written since
    /// `last_event` except log lines. One transaction.
    fn undo_change(
        &mut self,
        before: &CoreTask,
        last_event: EventId,
        keep_position: bool,
    ) -> Result<(), String> {
        let target = reverted(before, self.task(before.id), keep_position);
        self.store
            .revert_task(&target, last_event)
            .map_err(|e| e.to_string())
    }

    /// Remembers `before` (the task as it is now) for undo of a change about
    /// to be made. `keep_position`: see `Undo::Changed`.
    fn snapshot(&self, id: TaskId, keep_position: bool) -> Option<Undo> {
        let before = self.task(id)?.clone();
        let last_event = self.events.iter().map(|e| e.id).max().unwrap_or(0);
        Some(Undo::Changed {
            before: Box::new(before),
            last_event,
            keep_position,
        })
    }

    /// Stores a new task, remembers it for undo, and reloads.
    pub(super) fn add_task(&mut self, new: fortnight_core::domain::NewTask) {
        match self.store.add_task(new) {
            Ok(task) => self.undo.remember(Undo::Added(task.id)),
            Err(e) => self.error = Some(e.to_string()),
        }
        self.reload_tasks();
    }

    pub(crate) fn set_status(&mut self, id: TaskId, status: TaskStatus) {
        let changed = self.task(id).is_some_and(|t| t.status != status);
        let undo = self.snapshot(id, true);
        match self.store.set_status(id, status) {
            Ok(()) => {
                if let (true, Some(undo)) = (changed, undo) {
                    self.undo.remember(undo);
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.reload_tasks();
    }

    /// Moves task `id` to `day` at `position` (a Week-tab drop): plans it
    /// on that day if it was elsewhere. Undo puts both back.
    pub(crate) fn move_task(&mut self, id: TaskId, day: Date, position: f64) {
        if let Some(task) = self.task(id).cloned() {
            // Undo puts the old position back too.
            let undo = self.snapshot(id, false);
            let mut result = Ok(());
            if task.day() != Some(day) {
                // Dropping on a day plans it there; the
                // deadline stays as it is.
                result = self.store.update_task(&CoreTask {
                    planned_for: Some(day),
                    ..task
                });
            }
            let result = result.and_then(|()| self.store.set_position(id, position));
            match result {
                Ok(()) => {
                    if let Some(undo) = undo {
                        self.undo.remember(undo);
                    }
                }
                Err(e) => self.error = Some(e.to_string()),
            }
            self.reload_tasks();
        }
    }

    /// Saves a day's notes: its free-standing log lines are replaced by one
    /// holding `notes` (none if it is empty), stamped when the first of
    /// them was, else now on today, else noon on that day. Nothing is
    /// written when they already say the same.
    pub(crate) fn save_notes(&mut self, day: Date, notes: &str) {
        let old: Vec<(EventId, Timestamp, &str)> = self
            .events
            .iter()
            .filter(|e| e.task_id.is_none() && self.tz.to_datetime(e.at).date() == day)
            .filter_map(|e| Some((e.id, e.at, e.log_text()?.trim_end())))
            .collect();
        let at = match old.first() {
            Some((_, at, _)) => *at,
            None if day == self.today() => Timestamp::now(),
            None => self
                .tz
                .to_timestamp(day.at(12, 0, 0, 0))
                .unwrap_or_else(|_| Timestamp::now()),
        };
        let notes = notes.trim();
        let same = old
            .iter()
            .map(|(_, _, t)| *t)
            .collect::<Vec<_>>()
            .join("\n");
        if same == notes {
            return;
        }
        let ids: Vec<EventId> = old.iter().map(|(id, _, _)| *id).collect();
        let text = (!notes.is_empty()).then_some(notes);
        if let Err(e) = self.store.replace_logs(&ids, text, at) {
            self.error = Some(format!("Could not save the notes: {e}"));
        }
        self.reload_tasks();
    }

    /// Removes every deleted task for good (Delete all on the Today tab's
    /// Deleted section). One undo puts them all back.
    pub(crate) fn delete_all_deleted(&mut self) {
        let ids: Vec<TaskId> = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Dropped)
            .map(|t| t.id)
            .collect();
        let mut removed = Vec::new();
        for id in ids {
            match self.store.delete_task(id) {
                Ok(deleted) => removed.push(deleted),
                Err(e) => {
                    self.error = Some(e.to_string());
                    break;
                }
            }
        }
        if !removed.is_empty() {
            self.undo.remember(Undo::Deleted(removed));
        }
        self.reload_tasks();
    }

    pub(crate) fn day_action(&mut self, action: day::Action) -> Task<Message> {
        let result = match action {
            day::Action::None => return Task::none(),
            day::Action::SetStatus(id, status) => {
                self.set_status(id, status);
                return Task::none();
            }
            day::Action::Update(task) => {
                let undo = self.snapshot(task.id, true);
                let r = self.store.update_task(&task);
                if let (Ok(()), Some(undo)) = (&r, undo) {
                    self.undo.remember(undo);
                }
                r
            }
            day::Action::Delete(id) => match self.store.delete_task(id) {
                Ok(deleted) => {
                    self.undo.remember(Undo::Deleted(vec![deleted]));
                    Ok(())
                }
                Err(e) => Err(e),
            },
            day::Action::Add(new) => {
                self.add_task(new);
                return Task::none();
            }
            day::Action::Reorder(positions) => self.store.set_positions(&positions),
            day::Action::NewTask(day) => return self.open_new_task(day),
            day::Action::FocusNotes => return iced::widget::operation::focus(day::NOTES_ID),
            day::Action::OpenLink(url) => {
                if let Err(e) = open_link(&url) {
                    self.error = Some(e);
                }
                return Task::none();
            }
        };
        if let Err(e) = result {
            self.error = Some(e.to_string());
        }
        self.reload_tasks();
        Task::none()
    }
}

/// Opens a web or mail address from the notes with the system's handler.
/// Other schemes (`file:`, `javascript:`, …) are refused.
fn open_link(url: &str) -> Result<(), String> {
    let allowed = ["https://", "http://", "mailto:"]
        .iter()
        .any(|p| url.len() > p.len() && url[..p.len()].eq_ignore_ascii_case(p));
    if !allowed {
        return Err(format!("Only web and mail links open from notes: {url}"));
    }
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(url)
        .spawn()
        .map(drop)
        .map_err(|e| format!("Could not open {url}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fortnight_core::sqlite::SqliteStore;

    #[test]
    fn undoing_an_edit_keeps_a_later_drag() {
        use fortnight_core::domain::NewTask;
        let store = SqliteStore::open_in_memory().unwrap();
        let mut before = store
            .add_task(NewTask {
                title: "t".into(),
                ..NewTask::default()
            })
            .unwrap();
        before.position = 1.0;
        let mut now = before.clone();
        now.title = "edited".into();
        now.position = 5.0;
        let undone = reverted(&before, Some(&now), true);
        assert_eq!(undone.title, "t");
        assert_eq!(undone.position, 5.0);
        // A move's undo puts the old position back.
        assert_eq!(reverted(&before, Some(&now), false).position, 1.0);
        // The task is gone: nothing to keep.
        assert_eq!(reverted(&before, None, true).position, 1.0);
    }
}
