//! The storage interface. FROZEN after step 1: do not edit.
//!
//! `sqlite::SqliteStore` is the only implementation. Every method takes
//! `&self`: SQLite connections need no exclusive borrow, and that keeps the
//! app's state simple. Calls are synchronous and fast at this scale, so the
//! app runs them directly in `update`.

use crate::domain::{
    NewReport, NewTask, Report, ReportId, ReportKey, ReportKind, Settings, Task, TaskId,
};
use jiff::civil::Date;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(String),
    #[error("task {0} not found")]
    TaskNotFound(TaskId),
    #[error("report {0} not found")]
    ReportNotFound(ReportId),
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
    fn add_task(&self, new: NewTask) -> StoreResult<Task>;

    /// Saves the editable fields of `task` (title, notes, project, due_on)
    /// and bumps `updated_at`. Status changes go through `set_done`.
    /// Errors with `TaskNotFound` if the id does not exist.
    fn update_task(&self, task: &Task) -> StoreResult<()>;

    /// Marks a task done (`status = done`, `completed_at = now`) or reopens
    /// it (`status = todo`, `completed_at = NULL`). Bumps `updated_at`.
    fn set_done(&self, id: TaskId, done: bool) -> StoreResult<()>;

    /// Deletes a task permanently. Errors with `TaskNotFound` if absent.
    fn delete_task(&self, id: TaskId) -> StoreResult<()>;

    // ---- reports ----

    /// Inserts a new report row (regenerating never overwrites) with
    /// `created_at = now`, and returns it.
    fn insert_report(&self, new: NewReport) -> StoreResult<Report>;

    /// The most recently created report of this kind for this week, whatever
    /// its status.
    fn latest_report(&self, kind: ReportKind, week_start: Date) -> StoreResult<Option<Report>>;

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
