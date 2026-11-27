//! Task writes and undo: what the Today and Week tabs' actions and the
//! task dialog write to the store (add, status, edit, move, reorder,
//! delete, a day's notes), each remembered on the undo stack, and undo
//! itself (Ctrl/Cmd+Z).

use crate::{Message, Ready};
use fortnight_app::views::{day, week};
use fortnight_core::domain::{
    DeletedTask, EventId, Task as CoreTask, TaskId, TaskStatus, next_occurrence,
};
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
    /// wrote (ids above `last_event`), keeping log lines. If completing a
    /// recurring task spawned a next occurrence, `spawned` names it so undo
    /// removes it too.
    Changed {
        before: Box<CoreTask>,
        last_event: EventId,
        /// Keep the task's position as it is at undo time (a later drag
        /// stays), unless the change was a move, which set the position.
        keep_position: bool,
        spawned: Option<TaskId>,
    },
    /// Tasks were deleted for good (one, or all of Deleted at once); put
    /// them back with their history.
    Deleted(Vec<DeletedTask>),
}

/// One undone change that Shift+Ctrl/Cmd+Z (or Ctrl+Y) can re-apply.
#[derive(Debug, Clone)]
enum Redo {
    /// Put back an added task that undo removed.
    Add(DeletedTask),
    /// Re-apply a task state that undo reverted (plus any spawned recurring
    /// occurrence that undo removed).
    Change {
        after: Box<CoreTask>,
        keep_position: bool,
        respawn: Option<DeletedTask>,
    },
    /// Delete tasks that undo restored.
    Delete(Vec<TaskId>),
}

/// How many changes undo/redo remembers.
const UNDO_DEPTH: usize = 30;

/// The changes undo and redo can step through, capped at `UNDO_DEPTH`.
#[derive(Default)]
pub(crate) struct UndoStack {
    undo: Vec<Undo>,
    redo: Vec<Redo>,
}

impl UndoStack {
    /// Remembers a fresh change for undo and clears the redo stack.
    fn remember(&mut self, undo: Undo) {
        self.redo.clear();
        self.push_undo(undo);
    }

    fn push_undo(&mut self, undo: Undo) {
        self.undo.push(undo);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
    }

    fn push_redo(&mut self, redo: Redo) {
        self.redo.push(redo);
        if self.redo.len() > UNDO_DEPTH {
            self.redo.remove(0);
        }
    }

    fn pop(&mut self) -> Option<Undo> {
        self.undo.pop()
    }

    fn pop_redo(&mut self) -> Option<Redo> {
        self.redo.pop()
    }

    /// Puts back an entry whose undo failed, for another try.
    fn put_back(&mut self, undo: Undo) {
        self.undo.push(undo);
    }

    /// Puts back an entry whose redo failed, for another try.
    fn put_back_redo(&mut self, redo: Redo) {
        self.redo.push(redo);
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
        let redo = match &last {
            Undo::Added(id) => self.undo_add(*id).map(Redo::Add),
            Undo::Changed {
                before,
                last_event,
                keep_position,
                spawned,
            } => self.undo_change(before, *last_event, *keep_position, *spawned),
            Undo::Deleted(deleted) => {
                // Put back what can be; keep the rest for another try.
                let mut left = Vec::new();
                let mut restored_ids = Vec::new();
                let mut failed = None;
                for d in deleted {
                    if failed.is_none() {
                        match self.store.restore_task(d) {
                            Ok(()) => restored_ids.push(d.task.id),
                            Err(e) => failed = Some(e.to_string()),
                        }
                    }
                    if failed.is_some() {
                        left.push(d.clone());
                    }
                }
                match failed {
                    None => Ok(Redo::Delete(restored_ids)),
                    Some(e) => {
                        self.undo.put_back(Undo::Deleted(left));
                        self.error = Some(format!("Could not undo: {e}"));
                        self.reload_tasks();
                        return;
                    }
                }
            }
        };
        match redo {
            Ok(entry) => self.undo.push_redo(entry),
            Err(e) => {
                self.error = Some(format!("Could not undo: {e}"));
                // Keep the entry (and its saved copy) for another try.
                if !matches!(last, Undo::Added(_)) {
                    self.undo.put_back(last);
                }
            }
        }
        self.reload_tasks();
    }

    /// Re-applies the last undone change (Shift+Ctrl/Cmd+Z or Ctrl+Y).
    pub(super) fn redo(&mut self) {
        let Some(entry) = self.undo.pop_redo() else {
            return;
        };
        let undone = match &entry {
            Redo::Add(deleted) => self
                .store
                .restore_task(deleted)
                .map(|()| Undo::Added(deleted.task.id))
                .map_err(|e| e.to_string()),
            Redo::Change {
                after,
                keep_position,
                respawn,
            } => self.redo_change(after, *keep_position, respawn.as_ref()),
            Redo::Delete(ids) => {
                let mut removed = Vec::new();
                let mut failed = None;
                for &id in ids {
                    match self.store.delete_task(id) {
                        Ok(d) => removed.push(d),
                        Err(e) => {
                            failed = Some(e.to_string());
                            break;
                        }
                    }
                }
                match failed {
                    None => Ok(Undo::Deleted(removed)),
                    Some(e) => {
                        if !removed.is_empty() {
                            self.undo.push_undo(Undo::Deleted(removed));
                        }
                        Err(e)
                    }
                }
            }
        };
        match undone {
            Ok(u) => self.undo.push_undo(u),
            Err(e) => {
                self.error = Some(format!("Could not redo: {e}"));
                self.undo.put_back_redo(entry);
            }
        }
        self.reload_tasks();
    }

    /// Deletes a task just added, unless it has since gained history (a log
    /// line, an edit), which deleting would lose. One transaction.
    fn undo_add(&mut self, id: TaskId) -> Result<DeletedTask, String> {
        match self.store.delete_new_task(id) {
            Ok(Some(deleted)) => Ok(deleted),
            Ok(None) => Err("the task has changed since you added it. Delete it instead".into()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Puts the task back as `before` (keeping its current position unless
    /// `keep_position` is false), drops the events written since `last_event`
    /// except log lines, and removes any recurring task spawned by the change.
    fn undo_change(
        &mut self,
        before: &CoreTask,
        last_event: EventId,
        keep_position: bool,
        spawned: Option<TaskId>,
    ) -> Result<Redo, String> {
        let after = self
            .task(before.id)
            .cloned()
            .ok_or_else(|| format!("task {} no longer exists", before.id))?;
        let target = reverted(before, Some(&after), keep_position);
        self.store
            .revert_task(&target, last_event)
            .map_err(|e| e.to_string())?;
        let respawn = spawned.and_then(|sid| self.store.delete_new_task(sid).ok().flatten());
        Ok(Redo::Change {
            after: Box::new(after),
            keep_position,
            respawn,
        })
    }

    /// Re-applies `after` (and restores `respawn` if any) and returns the
    /// matching `Undo::Changed` entry.
    fn redo_change(
        &mut self,
        after: &CoreTask,
        keep_position: bool,
        respawn: Option<&DeletedTask>,
    ) -> Result<Undo, String> {
        let before = self
            .task(after.id)
            .cloned()
            .ok_or_else(|| format!("task {} no longer exists", after.id))?;
        let last_event = self.events.iter().map(|e| e.id).max().unwrap_or(0);
        let target = reverted(after, Some(&before), keep_position);
        self.store.update_task(&target).map_err(|e| e.to_string())?;
        if target.status != before.status {
            self.store
                .set_status(target.id, target.status)
                .map_err(|e| e.to_string())?;
        }
        if !keep_position && target.position != before.position {
            self.store
                .set_position(target.id, target.position)
                .map_err(|e| e.to_string())?;
        }
        let spawned = if let Some(d) = respawn {
            self.store.restore_task(d).map_err(|e| e.to_string())?;
            Some(d.task.id)
        } else {
            None
        };
        Ok(Undo::Changed {
            before: Box::new(before),
            last_event,
            keep_position,
            spawned,
        })
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
            spawned: None,
        })
    }

    /// If `before` was an open recurring task, spawns its next occurrence
    /// (unless an open matching occurrence already exists) and returns its id.
    fn maybe_spawn_next(&mut self, before: &CoreTask, today: Date) -> Option<TaskId> {
        if !before.status.is_open() {
            return None;
        }
        let next = next_occurrence(before, today)?;
        let exists = self.tasks.iter().any(|t| {
            t.status.is_open()
                && t.title == next.title
                && t.recurrence == next.recurrence
                && t.planned_for == next.planned_for
        });
        if exists {
            return None;
        }
        match self.store.add_task(next) {
            Ok(task) => Some(task.id),
            Err(e) => {
                self.error = Some(format!("Could not create next occurrence: {e}"));
                None
            }
        }
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
        let before = self.task(id).cloned();
        let changed = before.as_ref().is_some_and(|t| t.status != status);
        let undo = self.snapshot(id, true);
        match self.store.set_status(id, status) {
            Ok(()) => {
                let spawned = if changed && status == TaskStatus::Done {
                    let today = self.today();
                    before
                        .as_ref()
                        .and_then(|t| self.maybe_spawn_next(t, today))
                } else {
                    None
                };
                if let (true, Some(mut undo)) = (changed, undo) {
                    if let Undo::Changed { spawned: s, .. } = &mut undo {
                        *s = spawned;
                    }
                    self.undo.remember(undo);
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.reload_tasks();
    }

    /// [`set_status`](Self::set_status) from the Today tab. A task ticked
    /// off today's list while it shows today, carried over from an earlier
    /// day or overdue (planned for another day), was done today: it is
    /// planned for today too, so it lands in today's Completed instead of
    /// going to the day it was planned for (out of sight). One undo puts
    /// both back.
    fn set_status_from_today(&mut self, id: TaskId, status: TaskStatus) {
        let today = self.today();
        let replan = self
            .task(id)
            .is_some_and(|t| plans_today_when_done(t, status, self.selected, today));
        if !replan {
            self.set_status(id, status);
            return;
        }
        let last_event = self.events.iter().map(|e| e.id).max().unwrap_or(0);
        // One write of just the day and the status, so an edit made
        // elsewhere since the last reload is kept; undo puts back the task
        // as the store had it.
        match self.store.complete_on(id, today) {
            Ok(before) => {
                let spawned = self.maybe_spawn_next(&before, today);
                self.undo.remember(Undo::Changed {
                    before: Box::new(before),
                    last_event,
                    keep_position: true,
                    spawned,
                });
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

    /// Moves task `id` to `day` and renumbers that day's open tasks when
    /// adjacent positions collided.
    pub(crate) fn move_task_and_reorder(
        &mut self,
        id: TaskId,
        day: Date,
        positions: &[(TaskId, f64)],
    ) {
        if let Some(task) = self.task(id).cloned() {
            let undo = self.snapshot(id, false);
            let mut result = Ok(());
            if task.day() != Some(day) {
                result = self.store.update_task(&CoreTask {
                    planned_for: Some(day),
                    ..task
                });
            }
            let result = result.and_then(|()| self.store.set_positions(positions));
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
    /// Saves `day`'s notes; `false` if the store refused them (the error
    /// is shown).
    pub(crate) fn save_notes(&mut self, day: Date, notes: &str) -> bool {
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
            return true;
        }
        let ids: Vec<EventId> = old.iter().map(|(id, _, _)| *id).collect();
        let text = (!notes.is_empty()).then_some(notes);
        let saved = match self.store.replace_logs(&ids, text, at) {
            Ok(_) => true,
            Err(e) => {
                self.error = Some(format!("Could not save the notes: {e}"));
                false
            }
        };
        self.reload_tasks();
        saved
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

    /// Carries out what the Week tab asked for.
    pub(crate) fn week_action(&mut self, action: week::Action) -> Task<Message> {
        match action {
            week::Action::None => {
                self.local = true;
                Task::none()
            }
            week::Action::Move(id, day, position) => {
                self.move_task(id, day, position);
                Task::none()
            }
            week::Action::MoveAndReorder(id, day, positions) => {
                self.move_task_and_reorder(id, day, &positions);
                Task::none()
            }
            week::Action::SetStatus(id, status) => {
                self.set_status(id, status);
                Task::none()
            }
            week::Action::NewTask(day) => self.open_new_task(day),
            week::Action::Edit(id) => self.open_edit(id),
            week::Action::OpenDay(day) => self.show(crate::Tab::Today, Some(day)),
            week::Action::Panel(action) => self.panel_action(action),
        }
    }

    pub(crate) fn day_action(&mut self, action: day::Action) -> Task<Message> {
        let result = match action {
            day::Action::None => return Task::none(),
            day::Action::SetStatus(id, status) => {
                self.set_status_from_today(id, status);
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
            day::Action::Add(mut new) => {
                // Quick add: no due date typed, so the setting's.
                if self.settings.due_on_added_day && new.due_on.is_none() {
                    new.due_on = new.planned_for;
                }
                self.add_task(new);
                return Task::none();
            }
            day::Action::Reorder(positions) => self.store.set_positions(&positions),
            day::Action::NewTask(day) => return self.open_new_task(day),
            day::Action::Focus(id) => return iced::widget::operation::focus(id),
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
    let allowed = ["https://", "http://", "mailto:"].iter().any(|p| {
        url.len() > p.len()
            && url
                .get(..p.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(p))
    });
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

/// Whether `task`, moved to `status` on the Today tab showing `shown`,
/// is planned for `today` too: see [`Ready::set_status_from_today`]. A
/// task with no planned day stays so (it shows on the day it was done).
fn plans_today_when_done(task: &CoreTask, status: TaskStatus, shown: Date, today: Date) -> bool {
    status == TaskStatus::Done
        && shown == today
        && task.day().is_some_and(|d| d != today)
        && (task.on_today_list(today) || task.is_overdue(today))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fortnight_core::sqlite::SqliteStore;

    #[test]
    fn ticked_off_today_a_carried_over_or_overdue_task_is_planned_today() {
        use jiff::civil::date;
        let today = date(2026, 11, 18);
        let task = |planned: Option<Date>, due: Option<Date>| CoreTask {
            planned_for: planned,
            due_on: due,
            ..CoreTask::default()
        };
        let done = |t: &CoreTask| plans_today_when_done(t, TaskStatus::Done, today, today);
        // Carried over from Monday.
        assert!(done(&task(Some(date(2026, 11, 16)), None)));
        // Planned for Friday but overdue.
        assert!(done(&task(
            Some(date(2026, 11, 20)),
            Some(date(2026, 11, 17))
        )));
        // Already today's, planned later and not overdue, or undated.
        assert!(!done(&task(Some(today), None)));
        assert!(!done(&task(Some(date(2026, 11, 20)), None)));
        assert!(!done(&task(None, Some(date(2026, 11, 17)))));
        // Only on ticking, only with today shown.
        let carried = task(Some(date(2026, 11, 16)), None);
        assert!(!plans_today_when_done(
            &carried,
            TaskStatus::Todo,
            today,
            today
        ));
        assert!(!plans_today_when_done(
            &carried,
            TaskStatus::Done,
            date(2026, 11, 16),
            today
        ));
    }

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

    #[test]
    fn open_link_rejects_non_ascii_schemes_without_panicking() {
        assert!(open_link("日本語://example").is_err());
        assert!(open_link("éééé://example").is_err());
        assert!(open_link("file:///etc/passwd").is_err());
        assert!(open_link("https://").is_err());
    }
}
