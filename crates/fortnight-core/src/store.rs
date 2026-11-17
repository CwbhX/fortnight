//! The storage interface. FROZEN (round 2): do not edit.
//!
//! `sqlite::SqliteStore` is the only implementation. Every method takes
//! `&self`: SQLite connections need no exclusive borrow, and that keeps the
//! app's state simple. Calls are synchronous and fast at this scale, so the
//! app runs them directly in `update`.
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

    /// Round 1 shorthand: `set_status(id, Done)` or `set_status(id, Todo)`.
    fn set_done(&self, id: TaskId, done: bool) -> StoreResult<()> {
        self.set_status(
            id,
            if done {
                TaskStatus::Done
            } else {
                TaskStatus::Todo
            },
        )
    }

    /// Deletes a task permanently, with its events, and returns both.
    /// Errors with `TaskNotFound` if absent.
    fn delete_task(&self, id: TaskId) -> StoreResult<DeletedTask>;

    /// Undo for `delete_task`: re-inserts the task and its events with their
    /// original ids and values. Writes no new event.
    fn restore_task(&self, deleted: &DeletedTask) -> StoreResult<()>;

    // ---- history ----

    /// Appends a log line at `now`, attached to `task_id` or free-standing.
    /// Errors with `TaskNotFound` if `task_id` is given and absent.
    fn add_log(&self, task_id: Option<TaskId>, text: &str) -> StoreResult<Event>;

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

    // ---- settings ----

    /// The saved settings, or `Settings::default()` if none are saved.
    fn load_settings(&self) -> StoreResult<Settings>;

    fn save_settings(&self, settings: &Settings) -> StoreResult<()>;
}
