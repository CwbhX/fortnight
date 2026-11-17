//! SQLite implementation of `Store`. Owner: agent A.
//!
//! Schema: `tasks`, `reports`, `settings` as in PLAN.md section 6 (the
//! `events` table is out of prototype scope). Rollback journal with
//! `synchronous=FULL`, not WAL. Schema versioned with `PRAGMA user_version`
//! (rusqlite_migration 2.7 requires rusqlite 0.41, so it is not used).
//!
//! Storage formats:
//! - Timestamps: RFC 3339 UTC with exactly nine fractional digits, so the
//!   strings sort in time order (`2026-11-16T09:00:00.000000000Z`).
//! - Dates: `YYYY-MM-DD`.
//! - Enums: the `as_str` strings from `domain`.
//! - Settings: the whole `Settings` as one JSON value under key `settings`.

use crate::domain::{
    DeletedTask, Event, EventId, NewReport, NewTask, Report, ReportId, ReportKey, ReportKind,
    ReportStatus, Settings, Task, TaskId, TaskStatus,
};
use crate::store::{Store, StoreError, StoreResult};
use jiff::Timestamp;
use jiff::civil::Date;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::path::Path;
use std::time::Duration;

/// Migration `i` takes the schema from `user_version = i` to `i + 1`.
/// Append new entries; never edit an applied one.
const MIGRATIONS: &[&str] = &[
    // 1: initial schema (PLAN.md section 6, minus `events`).
    "CREATE TABLE tasks (
        id               INTEGER PRIMARY KEY,
        title            TEXT    NOT NULL,
        notes            TEXT    NOT NULL DEFAULT '',
        status           TEXT    NOT NULL CHECK (status IN ('todo','doing','done','dropped')),
        project          TEXT,
        due_on           TEXT,
        planned_for      TEXT,
        estimate_minutes INTEGER,
        priority         INTEGER NOT NULL DEFAULT 0,
        private          INTEGER NOT NULL DEFAULT 0,
        position         REAL    NOT NULL,
        created_at       TEXT    NOT NULL,
        updated_at       TEXT    NOT NULL,
        completed_at     TEXT
    );
    CREATE TABLE reports (
        id             INTEGER PRIMARY KEY,
        kind           TEXT NOT NULL CHECK (kind IN ('summary','plan')),
        week_start     TEXT NOT NULL,
        status         TEXT NOT NULL,
        digest_json    TEXT NOT NULL,
        content_md     TEXT,
        edited_md      TEXT,
        model          TEXT,
        endpoint_host  TEXT,
        prompt_version INTEGER NOT NULL,
        error          TEXT,
        created_at     TEXT NOT NULL
    );
    CREATE INDEX reports_week ON reports (kind, week_start, created_at);
    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
];

const SETTINGS_KEY: &str = "settings";

const TASK_COLUMNS: &str =
    "id, title, notes, status, project, due_on, position, created_at, updated_at, completed_at";

const REPORT_COLUMNS: &str = "id, kind, week_start, status, digest_json, content_md, edited_md, \
     model, endpoint_host, prompt_version, error, created_at";

pub struct SqliteStore {
    conn: Connection,
}

impl SqliteStore {
    /// Opens (creating if needed) the database at `path` and runs migrations.
    pub fn open(path: &Path) -> StoreResult<Self> {
        Self::init(Connection::open(path).map_err(db)?)
    }

    /// An in-memory database with the schema applied. For tests.
    pub fn open_in_memory() -> StoreResult<Self> {
        Self::init(Connection::open_in_memory().map_err(db)?)
    }

    fn init(mut conn: Connection) -> StoreResult<Self> {
        conn.busy_timeout(Duration::from_secs(5)).map_err(db)?;
        conn.execute_batch(
            "PRAGMA journal_mode = DELETE;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;",
        )
        .map_err(db)?;
        migrate(&mut conn)?;
        Ok(SqliteStore { conn })
    }
}

/// Applies every migration past the current `user_version`, all in one
/// transaction.
fn migrate(conn: &mut Connection) -> StoreResult<()> {
    let tx = conn.transaction().map_err(db)?;
    let current: i64 = tx
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(db)?;
    let target = MIGRATIONS.len() as i64;
    if current < 0 || current > target {
        return Err(StoreError::Invalid(format!(
            "database schema version {current} is not one this app supports ({target})"
        )));
    }
    if current == target {
        return Ok(());
    }
    for sql in &MIGRATIONS[current as usize..] {
        tx.execute_batch(sql).map_err(db)?;
    }
    tx.pragma_update(None, "user_version", target).map_err(db)?;
    tx.commit().map_err(db)
}

fn db(e: rusqlite::Error) -> StoreError {
    StoreError::Db(e.to_string())
}

fn ts_to_sql(ts: Timestamp) -> String {
    format!("{ts:.9}")
}

fn ts_from_sql(s: &str) -> StoreResult<Timestamp> {
    s.parse()
        .map_err(|e| StoreError::Invalid(format!("bad timestamp {s:?}: {e}")))
}

fn date_to_sql(d: Date) -> String {
    d.to_string()
}

fn date_from_sql(s: &str) -> StoreResult<Date> {
    s.parse()
        .map_err(|e| StoreError::Invalid(format!("bad date {s:?}: {e}")))
}

/// Raw task row: column values as stored, converted in `into_task`.
struct TaskRow {
    id: i64,
    title: String,
    notes: String,
    status: String,
    project: Option<String>,
    due_on: Option<String>,
    position: f64,
    created_at: String,
    updated_at: String,
    completed_at: Option<String>,
}

impl TaskRow {
    fn from_row(r: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(TaskRow {
            id: r.get(0)?,
            title: r.get(1)?,
            notes: r.get(2)?,
            status: r.get(3)?,
            project: r.get(4)?,
            due_on: r.get(5)?,
            position: r.get(6)?,
            created_at: r.get(7)?,
            updated_at: r.get(8)?,
            completed_at: r.get(9)?,
        })
    }

    fn into_task(self) -> StoreResult<Task> {
        Ok(Task {
            id: self.id,
            title: self.title,
            notes: self.notes,
            status: TaskStatus::parse(&self.status).ok_or_else(|| {
                StoreError::Invalid(format!("task {}: bad status {:?}", self.id, self.status))
            })?,
            project: self.project,
            due_on: self.due_on.as_deref().map(date_from_sql).transpose()?,
            position: self.position,
            created_at: ts_from_sql(&self.created_at)?,
            updated_at: ts_from_sql(&self.updated_at)?,
            completed_at: self.completed_at.as_deref().map(ts_from_sql).transpose()?,
            // STUB (round 2 step 1): agent A reads these columns.
            ..Task::default()
        })
    }
}

struct ReportRow {
    id: i64,
    kind: String,
    week_start: String,
    status: String,
    digest_json: String,
    content_md: Option<String>,
    edited_md: Option<String>,
    model: Option<String>,
    endpoint_host: Option<String>,
    prompt_version: i64,
    error: Option<String>,
    created_at: String,
}

impl ReportRow {
    fn from_row(r: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(ReportRow {
            id: r.get(0)?,
            kind: r.get(1)?,
            week_start: r.get(2)?,
            status: r.get(3)?,
            digest_json: r.get(4)?,
            content_md: r.get(5)?,
            edited_md: r.get(6)?,
            model: r.get(7)?,
            endpoint_host: r.get(8)?,
            prompt_version: r.get(9)?,
            error: r.get(10)?,
            created_at: r.get(11)?,
        })
    }

    fn into_report(self) -> StoreResult<Report> {
        Ok(Report {
            id: self.id,
            kind: parse_kind(&self.kind)?,
            week_start: date_from_sql(&self.week_start)?,
            status: ReportStatus::parse(&self.status).ok_or_else(|| {
                StoreError::Invalid(format!("report {}: bad status {:?}", self.id, self.status))
            })?,
            digest_json: self.digest_json,
            content_md: self.content_md,
            edited_md: self.edited_md,
            model: self.model,
            endpoint_host: self.endpoint_host,
            prompt_version: self.prompt_version,
            error: self.error,
            created_at: ts_from_sql(&self.created_at)?,
        })
    }
}

fn parse_kind(s: &str) -> StoreResult<ReportKind> {
    ReportKind::parse(s).ok_or_else(|| StoreError::Invalid(format!("bad report kind {s:?}")))
}

impl SqliteStore {
    fn report_by_id(&self, id: ReportId) -> StoreResult<Option<Report>> {
        self.conn
            .query_row(
                &format!("SELECT {REPORT_COLUMNS} FROM reports WHERE id = ?1"),
                [id],
                ReportRow::from_row,
            )
            .optional()
            .map_err(db)?
            .map(ReportRow::into_report)
            .transpose()
    }
}

impl Store for SqliteStore {
    fn list_tasks(&self) -> StoreResult<Vec<Task>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks ORDER BY position, id"
            ))
            .map_err(db)?;
        let rows = stmt
            .query_map([], TaskRow::from_row)
            .map_err(db)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db)?;
        rows.into_iter().map(TaskRow::into_task).collect()
    }

    fn get_task(&self, id: TaskId) -> StoreResult<Option<Task>> {
        self.conn
            .query_row(
                &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
                [id],
                TaskRow::from_row,
            )
            .optional()
            .map_err(db)?
            .map(TaskRow::into_task)
            .transpose()
    }

    fn add_task(&self, new: NewTask) -> StoreResult<Task> {
        let now = ts_to_sql(Timestamp::now());
        // One statement, so the position is computed atomically.
        let id: i64 = self
            .conn
            .query_row(
                "INSERT INTO tasks (title, notes, status, project, due_on, position,
                                    created_at, updated_at)
                 VALUES (?1, ?2, 'todo', ?3, ?4,
                         (SELECT COALESCE(MAX(position) + 1.0, 1.0) FROM tasks),
                         ?5, ?5)
                 RETURNING id",
                params![
                    new.title,
                    new.notes,
                    new.project,
                    new.due_on.map(date_to_sql),
                    now
                ],
                |r| r.get(0),
            )
            .map_err(db)?;
        self.get_task(id)?
            .ok_or_else(|| StoreError::Db(format!("inserted task {id} vanished")))
    }

    fn update_task(&self, task: &Task) -> StoreResult<()> {
        let n = self
            .conn
            .execute(
                "UPDATE tasks SET title = ?2, notes = ?3, project = ?4, due_on = ?5,
                                  updated_at = ?6
                 WHERE id = ?1",
                params![
                    task.id,
                    task.title,
                    task.notes,
                    task.project,
                    task.due_on.map(date_to_sql),
                    ts_to_sql(Timestamp::now())
                ],
            )
            .map_err(db)?;
        if n == 0 {
            return Err(StoreError::TaskNotFound(task.id));
        }
        Ok(())
    }

    fn set_status(&self, id: TaskId, status: TaskStatus) -> StoreResult<()> {
        let now = ts_to_sql(Timestamp::now());
        let completed_at = (status == TaskStatus::Done).then_some(now.as_str());
        let n = self
            .conn
            .execute(
                "UPDATE tasks SET status = ?2, completed_at = ?3, updated_at = ?4 WHERE id = ?1",
                params![id, status.as_str(), completed_at, now],
            )
            .map_err(db)?;
        if n == 0 {
            return Err(StoreError::TaskNotFound(id));
        }
        Ok(())
    }

    fn delete_task(&self, id: TaskId) -> StoreResult<DeletedTask> {
        let task = self.get_task(id)?.ok_or(StoreError::TaskNotFound(id))?;
        let n = self
            .conn
            .execute("DELETE FROM tasks WHERE id = ?1", [id])
            .map_err(db)?;
        if n == 0 {
            return Err(StoreError::TaskNotFound(id));
        }
        Ok(DeletedTask {
            task,
            events: Vec::new(),
        })
    }

    // STUBS (round 2 step 1): agent A implements these.

    fn restore_task(&self, _deleted: &DeletedTask) -> StoreResult<()> {
        Err(StoreError::Db("restore_task is not implemented yet".into()))
    }

    fn add_log(&self, _task_id: Option<TaskId>, _text: &str) -> StoreResult<Event> {
        Err(StoreError::Db("log lines are not implemented yet".into()))
    }

    fn delete_log(&self, id: EventId) -> StoreResult<()> {
        Err(StoreError::EventNotFound(id))
    }

    fn task_events(&self, _id: TaskId) -> StoreResult<Vec<Event>> {
        Ok(Vec::new())
    }

    fn events_between(&self, _from: Timestamp, _to: Timestamp) -> StoreResult<Vec<Event>> {
        Ok(Vec::new())
    }

    fn list_reports(&self, kind: ReportKind, week_start: Date) -> StoreResult<Vec<Report>> {
        Ok(self.latest_report(kind, week_start)?.into_iter().collect())
    }

    fn all_reports(&self) -> StoreResult<Vec<Report>> {
        Ok(Vec::new())
    }

    fn insert_report(&self, new: NewReport) -> StoreResult<Report> {
        let id: i64 = self
            .conn
            .query_row(
                "INSERT INTO reports (kind, week_start, status, digest_json, content_md,
                                      model, endpoint_host, prompt_version, error, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 RETURNING id",
                params![
                    new.kind.as_str(),
                    date_to_sql(new.week_start),
                    new.status.as_str(),
                    new.digest_json,
                    new.content_md,
                    new.model,
                    new.endpoint_host,
                    new.prompt_version,
                    new.error,
                    ts_to_sql(Timestamp::now())
                ],
                |r| r.get(0),
            )
            .map_err(db)?;
        self.report_by_id(id)?
            .ok_or_else(|| StoreError::Db(format!("inserted report {id} vanished")))
    }

    fn latest_report(&self, kind: ReportKind, week_start: Date) -> StoreResult<Option<Report>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {REPORT_COLUMNS} FROM reports
                     WHERE kind = ?1 AND week_start = ?2
                     ORDER BY created_at DESC, id DESC
                     LIMIT 1"
                ),
                params![kind.as_str(), date_to_sql(week_start)],
                ReportRow::from_row,
            )
            .optional()
            .map_err(db)?
            .map(ReportRow::into_report)
            .transpose()
    }

    fn ready_report_keys(&self) -> StoreResult<Vec<ReportKey>> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT DISTINCT kind, week_start FROM reports
                 WHERE status = 'ready'
                 ORDER BY week_start, kind",
            )
            .map_err(db)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(db)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db)?;
        rows.into_iter()
            .map(|(kind, week)| {
                Ok(ReportKey {
                    kind: parse_kind(&kind)?,
                    week_start: date_from_sql(&week)?,
                })
            })
            .collect()
    }

    fn set_report_edit(&self, id: ReportId, edited_md: Option<&str>) -> StoreResult<()> {
        let n = self
            .conn
            .execute(
                "UPDATE reports SET edited_md = ?2 WHERE id = ?1",
                params![id, edited_md],
            )
            .map_err(db)?;
        if n == 0 {
            return Err(StoreError::ReportNotFound(id));
        }
        Ok(())
    }

    fn load_settings(&self) -> StoreResult<Settings> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [SETTINGS_KEY],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        match json {
            None => Ok(Settings::default()),
            Some(json) => serde_json::from_str(&json)
                .map_err(|e| StoreError::Invalid(format!("bad settings JSON: {e}"))),
        }
    }

    fn save_settings(&self, settings: &Settings) -> StoreResult<()> {
        let json = serde_json::to_string(settings)
            .map_err(|e| StoreError::Invalid(format!("cannot encode settings: {e}")))?;
        self.conn
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![SETTINGS_KEY, json],
            )
            .map_err(db)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn store() -> SqliteStore {
        SqliteStore::open_in_memory().unwrap()
    }

    fn new_task(title: &str) -> NewTask {
        NewTask {
            title: title.to_string(),
            ..NewTask::default()
        }
    }

    fn new_report(kind: ReportKind, week: Date, status: ReportStatus, text: &str) -> NewReport {
        NewReport {
            kind,
            week_start: week,
            status,
            digest_json: "{}".to_string(),
            content_md: (status == ReportStatus::Ready).then(|| text.to_string()),
            model: Some("m".to_string()),
            endpoint_host: Some("localhost".to_string()),
            prompt_version: 1,
            error: (status == ReportStatus::Failed).then(|| text.to_string()),
        }
    }

    #[test]
    fn task_crud_and_ordering() {
        let s = store();
        assert!(s.list_tasks().unwrap().is_empty());

        let a = s
            .add_task(NewTask {
                title: "a".into(),
                notes: "n".into(),
                project: Some("work".into()),
                due_on: Some(date(2026, 11, 20)),
                ..NewTask::default()
            })
            .unwrap();
        let b = s.add_task(new_task("b")).unwrap();
        let c = s.add_task(new_task("c")).unwrap();
        assert_eq!(a.position, 1.0);
        assert_eq!(b.position, 2.0);
        assert_eq!(c.position, 3.0);
        assert_eq!(a.status, TaskStatus::Todo);
        assert_eq!(a.created_at, a.updated_at);
        assert_eq!(a.completed_at, None);
        assert_eq!(s.get_task(a.id).unwrap(), Some(a.clone()));
        assert_eq!(a.due_on, Some(date(2026, 11, 20)));
        assert_eq!(a.project.as_deref(), Some("work"));

        let titles: Vec<_> = s
            .list_tasks()
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(titles, ["a", "b", "c"]);

        let mut edited = b.clone();
        edited.title = "b2".into();
        edited.notes = "notes".into();
        edited.project = Some("home".into());
        edited.due_on = Some(date(2026, 12, 1));
        s.update_task(&edited).unwrap();
        let got = s.get_task(b.id).unwrap().unwrap();
        assert_eq!(got.title, "b2");
        assert_eq!(got.notes, "notes");
        assert_eq!(got.project.as_deref(), Some("home"));
        assert_eq!(got.due_on, Some(date(2026, 12, 1)));
        assert!(got.updated_at >= b.updated_at);
        assert_eq!(got.created_at, b.created_at);

        edited.project = None;
        edited.due_on = None;
        s.update_task(&edited).unwrap();
        let got = s.get_task(b.id).unwrap().unwrap();
        assert_eq!((got.project, got.due_on), (None, None));

        s.delete_task(a.id).unwrap();
        assert_eq!(s.get_task(a.id).unwrap(), None);
        // Deleting the last task does not reuse its position.
        s.delete_task(c.id).unwrap();
        let d = s.add_task(new_task("d")).unwrap();
        assert_eq!(d.position, 3.0);
        let titles: Vec<_> = s
            .list_tasks()
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(titles, ["b2", "d"]);
    }

    #[test]
    fn set_done_sets_then_clears_completed_at() {
        let s = store();
        let t = s.add_task(new_task("t")).unwrap();

        s.set_done(t.id, true).unwrap();
        let done = s.get_task(t.id).unwrap().unwrap();
        assert!(done.is_done());
        let completed = done.completed_at.expect("completed_at set");
        assert!(completed >= t.created_at);
        assert_eq!(done.updated_at, completed);

        s.set_done(t.id, false).unwrap();
        let open = s.get_task(t.id).unwrap().unwrap();
        assert_eq!(open.status, TaskStatus::Todo);
        assert_eq!(open.completed_at, None);
    }

    #[test]
    fn missing_ids_are_not_found() {
        let s = store();
        let mut ghost = s.add_task(new_task("x")).unwrap();
        s.delete_task(ghost.id).unwrap();
        ghost.title = "y".into();
        let id = ghost.id;
        assert_eq!(s.update_task(&ghost), Err(StoreError::TaskNotFound(id)));
        assert_eq!(s.set_done(id, true), Err(StoreError::TaskNotFound(id)));
        assert_eq!(s.delete_task(id), Err(StoreError::TaskNotFound(id)));
        assert_eq!(
            s.set_report_edit(42, Some("x")),
            Err(StoreError::ReportNotFound(42))
        );
        assert_eq!(s.get_task(id).unwrap(), None);
    }

    #[test]
    fn unknown_stored_status_is_invalid() {
        let s = store();
        let t = s.add_task(new_task("t")).unwrap();
        // Bypass the CHECK constraint to simulate a corrupt or foreign file.
        s.conn
            .execute_batch("PRAGMA ignore_check_constraints = ON")
            .unwrap();
        s.conn
            .execute("UPDATE tasks SET status = 'later' WHERE id = ?1", [t.id])
            .unwrap();
        assert!(matches!(s.get_task(t.id), Err(StoreError::Invalid(_))));
        assert!(matches!(s.list_tasks(), Err(StoreError::Invalid(_))));
    }

    #[test]
    fn reports_insert_and_latest() {
        let s = store();
        let week = date(2026, 11, 16);
        assert_eq!(s.latest_report(ReportKind::Summary, week).unwrap(), None);

        let first = s
            .insert_report(new_report(
                ReportKind::Summary,
                week,
                ReportStatus::Ready,
                "one",
            ))
            .unwrap();
        assert_eq!(first.kind, ReportKind::Summary);
        assert_eq!(first.week_start, week);
        assert_eq!(first.content_md.as_deref(), Some("one"));
        assert_eq!(first.edited_md, None);
        assert_eq!(first.prompt_version, 1);
        assert_eq!(
            s.latest_report(ReportKind::Summary, week).unwrap(),
            Some(first.clone())
        );

        // A later failed attempt is the latest, whatever its status.
        let failed = s
            .insert_report(new_report(
                ReportKind::Summary,
                week,
                ReportStatus::Failed,
                "boom",
            ))
            .unwrap();
        let latest = s.latest_report(ReportKind::Summary, week).unwrap().unwrap();
        assert_eq!(latest.id, failed.id);
        assert_eq!(latest.status, ReportStatus::Failed);
        assert_eq!(latest.error.as_deref(), Some("boom"));
        assert_eq!(latest.text(), None);

        let second = s
            .insert_report(new_report(
                ReportKind::Summary,
                week,
                ReportStatus::Ready,
                "two",
            ))
            .unwrap();
        let latest = s.latest_report(ReportKind::Summary, week).unwrap().unwrap();
        assert_eq!(latest.id, second.id);
        assert_eq!(latest.text(), Some("two"));

        // Ties on created_at are broken by id.
        s.conn
            .execute(
                "UPDATE reports SET created_at = ?1",
                [ts_to_sql(first.created_at)],
            )
            .unwrap();
        let latest = s.latest_report(ReportKind::Summary, week).unwrap().unwrap();
        assert_eq!(latest.id, second.id);

        // Other kinds and weeks are separate.
        assert_eq!(s.latest_report(ReportKind::Plan, week).unwrap(), None);
        assert_eq!(
            s.latest_report(ReportKind::Summary, date(2026, 11, 9))
                .unwrap(),
            None
        );
    }

    #[test]
    fn ready_report_keys_excludes_failed_and_dedups() {
        let s = store();
        let w1 = date(2026, 11, 9);
        let w2 = date(2026, 11, 16);
        use ReportKind::*;
        use ReportStatus::*;
        for (kind, week, status) in [
            (Summary, w1, Ready),
            (Summary, w1, Ready),
            (Summary, w1, Failed),
            (Plan, w1, Failed),
            (Plan, w2, Failed),
            (Plan, w2, Ready),
            (Summary, w2, Failed),
        ] {
            s.insert_report(new_report(kind, week, status, "x"))
                .unwrap();
        }
        let mut keys = s.ready_report_keys().unwrap();
        keys.sort_by_key(|k| (k.week_start, k.kind.as_str()));
        assert_eq!(
            keys,
            [
                ReportKey {
                    kind: Summary,
                    week_start: w1
                },
                ReportKey {
                    kind: Plan,
                    week_start: w2
                },
            ]
        );
    }

    #[test]
    fn report_edit_preferred_then_cleared() {
        let s = store();
        let week = date(2026, 11, 16);
        let r = s
            .insert_report(new_report(
                ReportKind::Plan,
                week,
                ReportStatus::Ready,
                "model",
            ))
            .unwrap();
        assert_eq!(r.text(), Some("model"));

        s.set_report_edit(r.id, Some("mine")).unwrap();
        let got = s.latest_report(ReportKind::Plan, week).unwrap().unwrap();
        assert_eq!(got.edited_md.as_deref(), Some("mine"));
        assert_eq!(got.content_md.as_deref(), Some("model"));
        assert_eq!(got.text(), Some("mine"));

        s.set_report_edit(r.id, None).unwrap();
        let got = s.latest_report(ReportKind::Plan, week).unwrap().unwrap();
        assert_eq!(got.edited_md, None);
        assert_eq!(got.text(), Some("model"));
    }

    #[test]
    fn settings_default_and_round_trip() {
        let s = store();
        assert_eq!(s.load_settings().unwrap(), Settings::default());

        let mut settings = Settings::default();
        settings.llm.base_url = "http://localhost:11434/v1".into();
        settings.llm.model = "llama".into();
        settings.schedule.summary_day = jiff::civil::Weekday::Thursday;
        settings.schedule.working_days.pop();
        settings.summary_instructions = "short".into();
        s.save_settings(&settings).unwrap();
        assert_eq!(s.load_settings().unwrap(), settings);

        // Saving again overwrites rather than adding a row.
        settings.summary_instructions = "shorter".into();
        s.save_settings(&settings).unwrap();
        assert_eq!(s.load_settings().unwrap(), settings);
        let rows: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn timestamps_sort_as_strings() {
        let whole: Timestamp = "2026-11-16T10:00:00Z".parse().unwrap();
        let half: Timestamp = "2026-11-16T10:00:00.5Z".parse().unwrap();
        assert!(ts_to_sql(whole) < ts_to_sql(half));
        assert_eq!(ts_from_sql(&ts_to_sql(half)).unwrap(), half);
    }

    #[test]
    fn open_file_persists_across_reopen() {
        let path = std::env::temp_dir().join(format!(
            "fortnight-sqlite-test-{}-{}.db",
            std::process::id(),
            Timestamp::now().as_nanosecond()
        ));
        let _ = std::fs::remove_file(&path);

        let result = std::panic::catch_unwind(|| {
            let task_id = {
                let s = SqliteStore::open(&path).unwrap();
                s.save_settings(&Settings {
                    summary_instructions: "persisted".into(),
                    ..Settings::default()
                })
                .unwrap();
                s.add_task(new_task("keep me")).unwrap().id
            };

            let s = SqliteStore::open(&path).unwrap();
            let t = s.get_task(task_id).unwrap().unwrap();
            assert_eq!(t.title, "keep me");
            assert_eq!(s.load_settings().unwrap().summary_instructions, "persisted");

            let version: i64 = s
                .conn
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .unwrap();
            assert_eq!(version, MIGRATIONS.len() as i64);
            let mode: String = s
                .conn
                .pragma_query_value(None, "journal_mode", |r| r.get(0))
                .unwrap();
            assert_eq!(mode, "delete");
            let sync: i64 = s
                .conn
                .pragma_query_value(None, "synchronous", |r| r.get(0))
                .unwrap();
            assert_eq!(sync, 2, "synchronous=FULL");
            let fk: i64 = s
                .conn
                .pragma_query_value(None, "foreign_keys", |r| r.get(0))
                .unwrap();
            assert_eq!(fk, 1);
            let index: Option<String> = s
                .conn
                .query_row(
                    "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'reports_week'",
                    [],
                    |r| r.get(0),
                )
                .optional()
                .unwrap();
            assert_eq!(index.as_deref(), Some("reports_week"));
        });

        let _ = std::fs::remove_file(&path);
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn newer_schema_version_is_rejected() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(
            SqliteStore::init(conn),
            Err(StoreError::Invalid(_))
        ));
    }
}
