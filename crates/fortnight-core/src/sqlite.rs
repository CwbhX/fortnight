//! SQLite implementation of `Store`. Owner: agent A.
//!
//! Schema: `tasks`, `reports`, `settings` as in PLAN.md section 6 (the
//! `events` table is out of prototype scope). Rollback journal with
//! `synchronous=FULL`, not WAL. Schema versioned with `PRAGMA user_version`
//! (rusqlite_migration 2.7 requires rusqlite 0.41, so it is not used).

use crate::domain::{
    NewReport, NewTask, Report, ReportId, ReportKey, ReportKind, Settings, Task, TaskId,
};
use crate::store::{Store, StoreResult};
use jiff::civil::Date;
use std::path::Path;

pub struct SqliteStore {
    _conn: rusqlite::Connection,
}

impl SqliteStore {
    /// Opens (creating if needed) the database at `path` and runs migrations.
    pub fn open(_path: &Path) -> StoreResult<Self> {
        todo!("agent A")
    }

    /// An in-memory database with the schema applied. For tests.
    pub fn open_in_memory() -> StoreResult<Self> {
        todo!("agent A")
    }
}

impl Store for SqliteStore {
    fn list_tasks(&self) -> StoreResult<Vec<Task>> {
        todo!("agent A")
    }

    fn get_task(&self, _id: TaskId) -> StoreResult<Option<Task>> {
        todo!("agent A")
    }

    fn add_task(&self, _new: NewTask) -> StoreResult<Task> {
        todo!("agent A")
    }

    fn update_task(&self, _task: &Task) -> StoreResult<()> {
        todo!("agent A")
    }

    fn set_done(&self, _id: TaskId, _done: bool) -> StoreResult<()> {
        todo!("agent A")
    }

    fn delete_task(&self, _id: TaskId) -> StoreResult<()> {
        todo!("agent A")
    }

    fn insert_report(&self, _new: NewReport) -> StoreResult<Report> {
        todo!("agent A")
    }

    fn latest_report(&self, _kind: ReportKind, _week_start: Date) -> StoreResult<Option<Report>> {
        todo!("agent A")
    }

    fn ready_report_keys(&self) -> StoreResult<Vec<ReportKey>> {
        todo!("agent A")
    }

    fn set_report_edit(&self, _id: ReportId, _edited_md: Option<&str>) -> StoreResult<()> {
        todo!("agent A")
    }

    fn load_settings(&self) -> StoreResult<Settings> {
        todo!("agent A")
    }

    fn save_settings(&self, _settings: &Settings) -> StoreResult<()> {
        todo!("agent A")
    }
}
