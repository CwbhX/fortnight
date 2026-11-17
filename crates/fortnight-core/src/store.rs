//! The storage interface. FROZEN (round 2): do not edit. (Review wave 2:
//! gained the atomic compound writes `revert_task`, `delete_new_task`,
//! `replace_logs`, `set_positions`, plus `add_log_at` and `prune_reports`.)
//!
//! `sqlite::SqliteStore` is the only implementation. Every method takes
//! `&self`: SQLite connections need no exclusive borrow, and that keeps the
//! app's state simple. Calls are synchronous and fast at this scale, so the
//! app runs them directly in `update`. Every method is one transaction: it
//! applies fully or not at all. A change made of several steps (undo, saving
//! a day's notes, a reorder) uses the compound method made for it rather
//! than a sequence of calls.
//!
//! History: every task change writes `Event`s in the same transaction as the
//! change itself (see each method). Events are append-only, except that a
//! log line can be deleted and a deleted task takes its events with it
//! (returned in `DeletedTask` so undo can restore them).

use crate::domain::{
    DeletedTask, Event, EventId, NewReport, NewTask, Report, ReportId, ReportKey, ReportKind,
    Settings, Task, TaskId, TaskStatus,
};
use jiff::Timestamp;
use jiff::civil::Date;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(String),
    #[error("task {0} not found")]
    TaskNotFound(TaskId),
    #[error("report {0} not found")]
    ReportNotFound(ReportId),
    #[error("log line {0} not found")]
    EventNotFound(EventId),
    #[error("stored data is invalid: {0}")]
    Invalid(String),
}

pub type StoreResult<T> = Result<T, StoreError>;

pub trait Store {
    // ---- tasks ----

    /// Every task, done or not, ordered by `position` then `id`.
    fn list_tasks(&self) -> StoreResult<Vec<Task>>;

    fn get_task(&self, id: TaskId) -> StoreResult<Option<Task>>;

    /// Inserts a `Todo` task at the end of the list (largest position + 1)
    /// with `created_at = updated_at = now`, and returns it.
    /// Writes a `Created` event.
    fn add_task(&self, new: NewTask) -> StoreResult<Task>;

    /// Saves the editable fields of `task` (title, notes, project, due_on,
    /// planned_for, estimate_minutes, priority, private) and bumps
    /// `updated_at`. Status changes go through `set_status`.
    /// Writes `DueChanged` / `PlannedChanged` when those dates change, and
    /// one `Edited` event naming any other changed fields. Saving an
    /// unchanged task writes no event and does not bump `updated_at`.
    /// Errors with `TaskNotFound` if the id does not exist.
    fn update_task(&self, task: &Task) -> StoreResult<()>;

    /// Sets the status. `Done` sets `completed_at = now`; any other status
    /// clears it. Bumps `updated_at` and writes a `StatusChanged` event,
    /// unless the status is already `status` (then it does nothing).
    fn set_status(&self, id: TaskId, status: TaskStatus) -> StoreResult<()>;

    /// Sets the manual ordering (`Task::position`; lower sorts first). No
    /// event and no `updated_at` change: order isn't history. (Round 3, for
    /// dragging cards within a day.)
    fn set_position(&self, id: TaskId, position: f64) -> StoreResult<()>;

    /// `set_position` for several tasks at once, all or nothing (a reorder).
    /// Errors with `Invalid` for a non-finite position and `TaskNotFound`
    /// for a missing id; then nothing changes.
    fn set_positions(&self, positions: &[(TaskId, f64)]) -> StoreResult<()>;

    /// Deletes a task permanently, with its events, and returns both.
    /// Errors with `TaskNotFound` if absent.
    fn delete_task(&self, id: TaskId) -> StoreResult<DeletedTask>;

    /// Undo for `delete_task`: re-inserts the task and its events with their
    /// original ids and values. Writes no new event.
    fn restore_task(&self, deleted: &DeletedTask) -> StoreResult<()>;

    /// Undo of a change: puts task `before.id` back exactly as `before`
    /// (every column, `status`, `completed_at`, `position` and `updated_at`
    /// included) and deletes its events with an id above `keep_up_to`,
    /// except log lines. In one transaction, without deleting the task, so
    /// a failure leaves it as it was. The result is what deleting the task
    /// and restoring `before` with its events up to `keep_up_to` plus all
    /// its log lines would leave. Writes no new event.
    /// Errors with `TaskNotFound` if the task no longer exists.
    fn revert_task(&self, before: &Task, keep_up_to: EventId) -> StoreResult<()>;

    /// Undo of `add_task`: deletes the task, with its events, only while its
    /// history is nothing but its `Created` event. Returns what it deleted,
    /// or `None` (changing nothing) if the task has gained other history (a
    /// log line, an edit, a status change) that deleting would lose.
    /// Errors with `TaskNotFound` if absent.
    fn delete_new_task(&self, id: TaskId) -> StoreResult<Option<DeletedTask>>;

    // ---- history ----

    /// Appends a log line stamped `at`, attached to `task_id` or
    /// free-standing. Errors with `TaskNotFound` if `task_id` is given and
    /// absent.
    fn add_log_at(&self, task_id: Option<TaskId>, text: &str, at: Timestamp) -> StoreResult<Event>;

    /// `add_log_at` stamped now.
    fn add_log(&self, task_id: Option<TaskId>, text: &str) -> StoreResult<Event> {
        self.add_log_at(task_id, text, Timestamp::now())
    }

    /// Replaces log lines (a day's notes): deletes every log line in
    /// `delete`, then, if `text` is `Some`, appends one free-standing log
    /// line with that text stamped `at`, and returns it. All or nothing:
    /// errors with `EventNotFound` (and changes nothing) if an id in
    /// `delete` is absent or not a log line.
    fn replace_logs(
        &self,
        delete: &[EventId],
        text: Option<&str>,
        at: Timestamp,
    ) -> StoreResult<Option<Event>>;

    /// Deletes a log line. Errors with `EventNotFound` if `id` is absent or
    /// is not a log line (other events cannot be deleted).
    fn delete_log(&self, id: EventId) -> StoreResult<()>;

    /// Every event of one task, oldest first (by `at`, then `id`).
    fn task_events(&self, id: TaskId) -> StoreResult<Vec<Event>>;

    /// Every event with `from <= at < to`, oldest first (by `at`, then
    /// `id`). `events_between(Timestamp::MIN, Timestamp::MAX)` lists all.
    fn events_between(&self, from: Timestamp, to: Timestamp) -> StoreResult<Vec<Event>>;

    // ---- reports ----

    /// Inserts a new report row (regenerating never overwrites) with
    /// `created_at = now`, and returns it.
    fn insert_report(&self, new: NewReport) -> StoreResult<Report>;

    /// The most recently created report of this kind for this week, whatever
    /// its status.
    fn latest_report(&self, kind: ReportKind, week_start: Date) -> StoreResult<Option<Report>>;

    /// Every version of the report of this kind for this week, whatever its
    /// status, newest first (by `created_at`, then `id`).
    fn list_reports(&self, kind: ReportKind, week_start: Date) -> StoreResult<Vec<Report>>;

    /// Every report row, ordered by `week_start`, `kind`, `created_at`.
    /// For export.
    fn all_reports(&self) -> StoreResult<Vec<Report>>;

    /// Distinct (kind, week) pairs that have at least one `Ready` report.
    /// This is what `week::due_reports` needs.
    fn ready_report_keys(&self) -> StoreResult<Vec<ReportKey>>;

    /// Sets (or with `None`, clears) the user's edit of a report.
    fn set_report_edit(&self, id: ReportId, edited_md: Option<&str>) -> StoreResult<()>;

    /// Deletes old versions of the report of this kind for this week: all
    /// but the newest `keep` (by `created_at`, then `id`), except that a
    /// version with an edit (`edited_md` set) and the newest `Ready`
    /// version are never deleted. One transaction. Returns how many rows
    /// it deleted.
    fn prune_reports(&self, kind: ReportKind, week_start: Date, keep: usize) -> StoreResult<usize>;

    // ---- settings ----

    /// The saved settings, or `Settings::default()` if none are saved.
    fn load_settings(&self) -> StoreResult<Settings>;

    fn save_settings(&self, settings: &Settings) -> StoreResult<()>;
}
