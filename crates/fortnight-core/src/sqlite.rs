//! SQLite implementation of `Store`. Owner: agent A.
//!
//! Schema: `tasks`, `events`, `reports`, `settings` as in PLAN.md section 6.
//! Rollback journal with `synchronous=FULL`, not WAL. Schema versioned with
//! `PRAGMA user_version` (rusqlite_migration 2.7 requires rusqlite 0.41, so
//! it is not used). All pending migrations run in one transaction, so a
//! migration either fully applies or leaves the file as it was.
//!
//! Storage formats:
//! - Timestamps: RFC 3339 UTC with exactly nine fractional digits, so the
//!   strings sort in time order (`2026-11-16T09:00:00.000000000Z`).
//! - Dates: `YYYY-MM-DD`.
//! - Enums: the `as_str` strings from `domain`.
//! - Events: `kind` is `EventKind::as_str`, `payload` is the `EventKind` as
//!   JSON (internally tagged, so it repeats `kind`).
//! - Settings: the whole `Settings` as one JSON value under key `settings`.
//!
//! History: every change and its events are written in one transaction.

use crate::domain::{
    DeletedTask, Event, EventId, EventKind, NewReport, NewTask, Report, ReportId, ReportKey,
    ReportKind, ReportStatus, Settings, Task, TaskId, TaskStatus,
};
use crate::store::{Store, StoreError, StoreResult};
use jiff::Timestamp;
use jiff::civil::Date;
use rusqlite::{
    Connection, OptionalExtension, Params, Row, Transaction, TransactionBehavior, params,
};
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
    // 2: rebuild `tasks` (same columns and values) with AUTOINCREMENT, and
    // add the append-only history, also AUTOINCREMENT. Ids are then never
    // reused, so undo can always put a deleted task and its events back
    // under their original ids, even after new tasks were added.
    //
    // The history is backfilled from existing tasks: a `created` event at
    // `created_at`, and for tasks that are no longer `todo` (round 1 only
    // had `done`) a `status_changed` from `todo` at `completed_at` (or
    // `updated_at` if that is missing). The payloads are the JSON
    // `serde_json` writes for the same `EventKind`.
    "CREATE TABLE tasks_v2 (
        id               INTEGER PRIMARY KEY AUTOINCREMENT,
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
    INSERT INTO tasks_v2 (id, title, notes, status, project, due_on, planned_for,
                          estimate_minutes, priority, private, position,
                          created_at, updated_at, completed_at)
        SELECT id, title, notes, status, project, due_on, planned_for,
               estimate_minutes, priority, private, position,
               created_at, updated_at, completed_at
        FROM tasks;
    DROP TABLE tasks;
    ALTER TABLE tasks_v2 RENAME TO tasks;
    CREATE TABLE events (
        id      INTEGER PRIMARY KEY AUTOINCREMENT,
        task_id INTEGER REFERENCES tasks(id),
        at      TEXT NOT NULL,
        kind    TEXT NOT NULL,
        payload TEXT NOT NULL
    );
    CREATE INDEX events_at ON events (at);
    CREATE INDEX events_task ON events (task_id);
    INSERT INTO events (task_id, at, kind, payload)
        SELECT id, created_at, 'created', json_object('kind', 'created', 'title', title)
        FROM tasks
        ORDER BY created_at, id;
    INSERT INTO events (task_id, at, kind, payload)
        SELECT id, COALESCE(completed_at, updated_at), 'status_changed',
               json_object('kind', 'status_changed', 'from', 'todo', 'to', status)
        FROM tasks
        WHERE status <> 'todo'
        ORDER BY COALESCE(completed_at, updated_at), id;",
    // 3: `Task::day` became the planned day only (it used to fall back to
    // the deadline). Copy the deadline into an empty planned day so every
    // task stays on the day it was shown on.
    "UPDATE tasks SET planned_for = due_on WHERE planned_for IS NULL AND due_on IS NOT NULL;",
];

const SETTINGS_KEY: &str = "settings";

const TASK_COLUMNS: &str = "id, title, notes, status, project, due_on, planned_for, \
     estimate_minutes, priority, private, position, created_at, updated_at, completed_at";

const EVENT_COLUMNS: &str = "id, task_id, at, kind, payload";

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
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
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

/// A bound for a range query on a timestamp column. Stored timestamps have
/// four-digit years, but timestamps outside years 0000..=9999 (such as
/// `Timestamp::MIN`) format with a sign and more digits and would not
/// compare correctly as strings, so they become an open end instead.
fn range_bound(ts: Timestamp) -> RangeBound {
    let s = ts_to_sql(ts);
    match s.as_bytes().first() {
        Some(b'-') => RangeBound::BeforeAll,
        Some(b'+') => RangeBound::AfterAll,
        _ => RangeBound::At(s),
    }
}

enum RangeBound {
    BeforeAll,
    At(String),
    AfterAll,
}

/// Raw task row: column values as stored, converted in `into_task`.
struct TaskRow {
    id: i64,
    title: String,
    notes: String,
    status: String,
    project: Option<String>,
    due_on: Option<String>,
    planned_for: Option<String>,
    estimate_minutes: Option<i64>,
    priority: i64,
    private: bool,
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
            planned_for: r.get(6)?,
            estimate_minutes: r.get(7)?,
            priority: r.get(8)?,
            private: r.get(9)?,
            position: r.get(10)?,
            created_at: r.get(11)?,
            updated_at: r.get(12)?,
            completed_at: r.get(13)?,
        })
    }

    fn into_task(self) -> StoreResult<Task> {
        let id = self.id;
        let invalid = |what: &str| StoreError::Invalid(format!("task {id}: bad {what}"));
        Ok(Task {
            id,
            title: self.title,
            notes: self.notes,
            status: TaskStatus::parse(&self.status)
                .ok_or_else(|| invalid(&format!("status {:?}", self.status)))?,
            project: self.project,
            due_on: self.due_on.as_deref().map(date_from_sql).transpose()?,
            planned_for: self.planned_for.as_deref().map(date_from_sql).transpose()?,
            estimate_minutes: self
                .estimate_minutes
                .map(u32::try_from)
                .transpose()
                .map_err(|_| invalid("estimate"))?,
            priority: u8::try_from(self.priority).map_err(|_| invalid("priority"))?,
            private: self.private,
            position: self.position,
            created_at: ts_from_sql(&self.created_at)?,
            updated_at: ts_from_sql(&self.updated_at)?,
            completed_at: self.completed_at.as_deref().map(ts_from_sql).transpose()?,
        })
    }
}

struct EventRow {
    id: i64,
    task_id: Option<i64>,
    at: String,
    kind: String,
    payload: String,
}

impl EventRow {
    fn from_row(r: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(EventRow {
            id: r.get(0)?,
            task_id: r.get(1)?,
            at: r.get(2)?,
            kind: r.get(3)?,
            payload: r.get(4)?,
        })
    }

    fn into_event(self) -> StoreResult<Event> {
        let kind: EventKind = serde_json::from_str(&self.payload)
            .map_err(|e| StoreError::Invalid(format!("event {}: bad payload: {e}", self.id)))?;
        if kind.as_str() != self.kind {
            return Err(StoreError::Invalid(format!(
                "event {}: kind {:?} does not match its payload",
                self.id, self.kind
            )));
        }
        Ok(Event {
            id: self.id,
            task_id: self.task_id,
            at: ts_from_sql(&self.at)?,
            kind,
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

// ---- helpers that work on a connection or an open transaction ----

fn query_tasks<P: Params>(conn: &Connection, where_order: &str, p: P) -> StoreResult<Vec<Task>> {
    let mut stmt = conn
        .prepare_cached(&format!("SELECT {TASK_COLUMNS} FROM tasks {where_order}"))
        .map_err(db)?;
    let rows = stmt
        .query_map(p, TaskRow::from_row)
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    rows.into_iter().map(TaskRow::into_task).collect()
}

fn query_task(conn: &Connection, id: TaskId) -> StoreResult<Option<Task>> {
    Ok(query_tasks(conn, "WHERE id = ?1", [id])?.pop())
}

fn query_events<P: Params>(conn: &Connection, where_clause: &str, p: P) -> StoreResult<Vec<Event>> {
    let mut stmt = conn
        .prepare_cached(&format!(
            "SELECT {EVENT_COLUMNS} FROM events {where_clause} ORDER BY at, id"
        ))
        .map_err(db)?;
    let rows = stmt
        .query_map(p, EventRow::from_row)
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    rows.into_iter().map(EventRow::into_event).collect()
}

fn task_exists(conn: &Connection, id: TaskId) -> StoreResult<bool> {
    conn.query_row("SELECT 1 FROM tasks WHERE id = ?1", [id], |_| Ok(()))
        .optional()
        .map(|r| r.is_some())
        .map_err(db)
}

fn payload(kind: &EventKind) -> StoreResult<String> {
    serde_json::to_string(kind)
        .map_err(|e| StoreError::Invalid(format!("cannot encode event: {e}")))
}

/// Appends an event and returns its id.
fn insert_event(
    conn: &Connection,
    task_id: Option<TaskId>,
    at: &str,
    kind: &EventKind,
) -> StoreResult<EventId> {
    conn.query_row(
        "INSERT INTO events (task_id, at, kind, payload) VALUES (?1, ?2, ?3, ?4) RETURNING id",
        params![task_id, at, kind.as_str(), payload(kind)?],
        |r| r.get(0),
    )
    .map_err(db)
}

impl SqliteStore {
    /// A write transaction that takes the write lock up front (`BEGIN
    /// IMMEDIATE`), so a busy database waits out the busy timeout instead of
    /// failing on a read-to-write upgrade when the GUI and CLI write at once.
    fn write_tx(&self) -> StoreResult<Transaction<'_>> {
        Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate).map_err(db)
    }

    /// `Store::add_log` at a given time instead of now: the Today tab's
    /// notes for a day other than today are stamped on that day.
    pub fn add_log_at(
        &self,
        task_id: Option<TaskId>,
        text: &str,
        at: Timestamp,
    ) -> StoreResult<Event> {
        let tx = self.write_tx()?;
        if let Some(id) = task_id
            && !task_exists(&tx, id)?
        {
            return Err(StoreError::TaskNotFound(id));
        }
        let kind = EventKind::Log {
            text: text.to_string(),
        };
        let id = insert_event(&tx, task_id, &ts_to_sql(at), &kind)?;
        tx.commit().map_err(db)?;
        // Nine fraction digits are lossless, so this equals a later read.
        Ok(Event {
            id,
            task_id,
            at,
            kind,
        })
    }

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

    fn query_reports<P: Params>(&self, rest: &str, p: P) -> StoreResult<Vec<Report>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT {REPORT_COLUMNS} FROM reports {rest}"))
            .map_err(db)?;
        let rows = stmt
            .query_map(p, ReportRow::from_row)
            .map_err(db)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db)?;
        rows.into_iter().map(ReportRow::into_report).collect()
    }
}

impl Store for SqliteStore {
    fn list_tasks(&self) -> StoreResult<Vec<Task>> {
        query_tasks(&self.conn, "ORDER BY position, id", [])
    }

    fn get_task(&self, id: TaskId) -> StoreResult<Option<Task>> {
        query_task(&self.conn, id)
    }

    fn add_task(&self, new: NewTask) -> StoreResult<Task> {
        let now = ts_to_sql(Timestamp::now());
        let tx = self.write_tx()?;
        let id: i64 = tx
            .query_row(
                "INSERT INTO tasks (title, notes, status, project, due_on, planned_for,
                                    estimate_minutes, priority, private, position,
                                    created_at, updated_at)
                 VALUES (?1, ?2, 'todo', ?3, ?4, ?5, ?6, ?7, ?8,
                         (SELECT COALESCE(MAX(position) + 1.0, 1.0) FROM tasks),
                         ?9, ?9)
                 RETURNING id",
                params![
                    new.title,
                    new.notes,
                    new.project,
                    new.due_on.map(date_to_sql),
                    new.planned_for.map(date_to_sql),
                    new.estimate_minutes,
                    new.priority,
                    new.private,
                    now
                ],
                |r| r.get(0),
            )
            .map_err(db)?;
        insert_event(
            &tx,
            Some(id),
            &now,
            &EventKind::Created {
                title: new.title.clone(),
            },
        )?;
        let task = query_task(&tx, id)?
            .ok_or_else(|| StoreError::Db(format!("inserted task {id} vanished")))?;
        tx.commit().map_err(db)?;
        Ok(task)
    }

    fn update_task(&self, task: &Task) -> StoreResult<()> {
        let tx = self.write_tx()?;
        let old = query_task(&tx, task.id)?.ok_or(StoreError::TaskNotFound(task.id))?;

        let mut events = Vec::new();
        if old.due_on != task.due_on {
            events.push(EventKind::DueChanged {
                from: old.due_on,
                to: task.due_on,
            });
        }
        if old.planned_for != task.planned_for {
            events.push(EventKind::PlannedChanged {
                from: old.planned_for,
                to: task.planned_for,
            });
        }
        let fields: Vec<String> = [
            ("title", old.title != task.title),
            ("notes", old.notes != task.notes),
            ("project", old.project != task.project),
            ("estimate", old.estimate_minutes != task.estimate_minutes),
            ("priority", old.priority != task.priority),
            ("private", old.private != task.private),
        ]
        .into_iter()
        .filter(|(_, changed)| *changed)
        .map(|(name, _)| name.to_string())
        .collect();
        if !fields.is_empty() {
            events.push(EventKind::Edited { fields });
        }
        if events.is_empty() {
            // Unchanged: no event, no `updated_at` bump.
            return Ok(());
        }

        let now = ts_to_sql(Timestamp::now());
        tx.execute(
            "UPDATE tasks SET title = ?2, notes = ?3, project = ?4, due_on = ?5,
                              planned_for = ?6, estimate_minutes = ?7, priority = ?8,
                              private = ?9, updated_at = ?10
             WHERE id = ?1",
            params![
                task.id,
                task.title,
                task.notes,
                task.project,
                task.due_on.map(date_to_sql),
                task.planned_for.map(date_to_sql),
                task.estimate_minutes,
                task.priority,
                task.private,
                now
            ],
        )
        .map_err(db)?;
        for kind in &events {
            insert_event(&tx, Some(task.id), &now, kind)?;
        }
        tx.commit().map_err(db)
    }

    fn set_position(&self, id: TaskId, position: f64) -> StoreResult<()> {
        if !position.is_finite() {
            return Err(StoreError::Invalid(format!(
                "task {id}: bad position {position}"
            )));
        }
        let changed = self
            .conn
            .execute(
                "UPDATE tasks SET position = ?2 WHERE id = ?1",
                params![id, position],
            )
            .map_err(db)?;
        if changed == 0 {
            return Err(StoreError::TaskNotFound(id));
        }
        Ok(())
    }

    fn set_status(&self, id: TaskId, status: TaskStatus) -> StoreResult<()> {
        let tx = self.write_tx()?;
        let old: String = tx
            .query_row("SELECT status FROM tasks WHERE id = ?1", [id], |r| r.get(0))
            .optional()
            .map_err(db)?
            .ok_or(StoreError::TaskNotFound(id))?;
        let from = TaskStatus::parse(&old)
            .ok_or_else(|| StoreError::Invalid(format!("task {id}: bad status {old:?}")))?;
        if from == status {
            return Ok(());
        }
        let now = ts_to_sql(Timestamp::now());
        let completed_at = (status == TaskStatus::Done).then_some(now.as_str());
        tx.execute(
            "UPDATE tasks SET status = ?2, completed_at = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, status.as_str(), completed_at, now],
        )
        .map_err(db)?;
        insert_event(
            &tx,
            Some(id),
            &now,
            &EventKind::StatusChanged { from, to: status },
        )?;
        tx.commit().map_err(db)
    }

    fn delete_task(&self, id: TaskId) -> StoreResult<DeletedTask> {
        let tx = self.write_tx()?;
        let task = query_task(&tx, id)?.ok_or(StoreError::TaskNotFound(id))?;
        let events = query_events(&tx, "WHERE task_id = ?1", [id])?;
        // Events first: they reference the task.
        tx.execute("DELETE FROM events WHERE task_id = ?1", [id])
            .map_err(db)?;
        tx.execute("DELETE FROM tasks WHERE id = ?1", [id])
            .map_err(db)?;
        tx.commit().map_err(db)?;
        Ok(DeletedTask { task, events })
    }

    fn restore_task(&self, deleted: &DeletedTask) -> StoreResult<()> {
        let t = &deleted.task;
        let tx = self.write_tx()?;
        tx.execute(
            &format!(
                "INSERT INTO tasks ({TASK_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"
            ),
            params![
                t.id,
                t.title,
                t.notes,
                t.status.as_str(),
                t.project,
                t.due_on.map(date_to_sql),
                t.planned_for.map(date_to_sql),
                t.estimate_minutes,
                t.priority,
                t.private,
                t.position,
                ts_to_sql(t.created_at),
                ts_to_sql(t.updated_at),
                t.completed_at.map(ts_to_sql)
            ],
        )
        .map_err(db)?;
        for e in &deleted.events {
            tx.execute(
                &format!("INSERT INTO events ({EVENT_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5)"),
                params![
                    e.id,
                    e.task_id,
                    ts_to_sql(e.at),
                    e.kind.as_str(),
                    payload(&e.kind)?
                ],
            )
            .map_err(db)?;
        }
        tx.commit().map_err(db)
    }

    fn add_log(&self, task_id: Option<TaskId>, text: &str) -> StoreResult<Event> {
        self.add_log_at(task_id, text, Timestamp::now())
    }

    fn delete_log(&self, id: EventId) -> StoreResult<()> {
        let n = self
            .conn
            .execute("DELETE FROM events WHERE id = ?1 AND kind = 'log'", [id])
            .map_err(db)?;
        if n == 0 {
            return Err(StoreError::EventNotFound(id));
        }
        Ok(())
    }

    fn task_events(&self, id: TaskId) -> StoreResult<Vec<Event>> {
        query_events(&self.conn, "WHERE task_id = ?1", [id])
    }

    fn events_between(&self, from: Timestamp, to: Timestamp) -> StoreResult<Vec<Event>> {
        if from >= to {
            return Ok(Vec::new());
        }
        // An open end is expressed as a string that sorts before (the empty
        // string) or after (U+10FFFF) every stored timestamp.
        let lower = match range_bound(from) {
            RangeBound::BeforeAll => String::new(),
            RangeBound::At(s) => s,
            RangeBound::AfterAll => return Ok(Vec::new()),
        };
        let upper = match range_bound(to) {
            RangeBound::BeforeAll => return Ok(Vec::new()),
            RangeBound::At(s) => s,
            RangeBound::AfterAll => char::MAX.to_string(),
        };
        query_events(&self.conn, "WHERE at >= ?1 AND at < ?2", [lower, upper])
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
        Ok(self
            .query_reports(
                "WHERE kind = ?1 AND week_start = ?2
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                params![kind.as_str(), date_to_sql(week_start)],
            )?
            .pop())
    }

    fn list_reports(&self, kind: ReportKind, week_start: Date) -> StoreResult<Vec<Report>> {
        self.query_reports(
            "WHERE kind = ?1 AND week_start = ?2 ORDER BY created_at DESC, id DESC",
            params![kind.as_str(), date_to_sql(week_start)],
        )
    }

    fn all_reports(&self) -> StoreResult<Vec<Report>> {
        self.query_reports("ORDER BY week_start, kind, created_at, id", [])
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
    fn set_status_done_sets_then_clears_completed_at() {
        let s = store();
        let t = s.add_task(new_task("t")).unwrap();

        s.set_status(t.id, TaskStatus::Done).unwrap();
        let done = s.get_task(t.id).unwrap().unwrap();
        assert!(done.is_done());
        let completed = done.completed_at.expect("completed_at set");
        assert!(completed >= t.created_at);
        assert_eq!(done.updated_at, completed);

        s.set_status(t.id, TaskStatus::Todo).unwrap();
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
        assert_eq!(
            s.set_status(id, TaskStatus::Done),
            Err(StoreError::TaskNotFound(id))
        );
        assert_eq!(
            s.set_status(id, TaskStatus::Dropped),
            Err(StoreError::TaskNotFound(id))
        );
        assert_eq!(s.delete_task(id), Err(StoreError::TaskNotFound(id)));
        assert_eq!(s.add_log(Some(id), "x"), Err(StoreError::TaskNotFound(id)));
        assert!(s.task_events(id).unwrap().is_empty());
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

    /// The GUI and the CLI may write at the same time: with immediate
    /// write transactions, a writer waits for the lock instead of failing
    /// on a read-to-write upgrade (which no busy timeout can rescue).
    ///
    /// SQLite's busy handler polls rather than queues, so under this much
    /// contention one thread can be starved for longer than the app's 5 s
    /// timeout on a slow disk (Windows CI). The test therefore opens every
    /// connection up front and gives it a generous timeout: it checks that
    /// writers wait, not how long the wait takes.
    #[test]
    fn concurrent_writers_wait_instead_of_failing() {
        let path = std::env::temp_dir().join(format!(
            "fortnight-sqlite-concurrent-{}-{}.db",
            std::process::id(),
            Timestamp::now().as_nanosecond()
        ));
        let _ = std::fs::remove_file(&path);
        let id = SqliteStore::open(&path)
            .unwrap()
            .add_task(new_task("shared"))
            .unwrap()
            .id;
        let stores: Vec<_> = (0..4)
            .map(|_| {
                let s = SqliteStore::open(&path).unwrap();
                s.conn.busy_timeout(Duration::from_secs(120)).unwrap();
                s
            })
            .collect();
        let threads: Vec<_> = stores
            .into_iter()
            .enumerate()
            .map(|(n, s)| {
                std::thread::spawn(move || {
                    let mut failures = 0;
                    for i in 0..100 {
                        let mut t = s.get_task(id).unwrap().unwrap();
                        t.notes = format!("{n}-{i}");
                        if s.update_task(&t).is_err() {
                            failures += 1;
                        }
                    }
                    failures
                })
            })
            .collect();
        let failures: i32 = threads.into_iter().map(|t| t.join().unwrap()).sum();
        let _ = std::fs::remove_file(&path);
        assert_eq!(failures, 0);
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

    // ---------------------------------------------------------- round 2

    use TaskStatus::{Doing, Done, Dropped, Todo};

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn kinds(events: &[Event]) -> Vec<EventKind> {
        events.iter().map(|e| e.kind.clone()).collect()
    }

    fn all_events(s: &SqliteStore) -> Vec<Event> {
        s.events_between(Timestamp::MIN, Timestamp::MAX).unwrap()
    }

    fn user_version(conn: &Connection) -> i64 {
        conn.pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap()
    }

    fn created(title: &str) -> EventKind {
        EventKind::Created {
            title: title.into(),
        }
    }

    fn status(from: TaskStatus, to: TaskStatus) -> EventKind {
        EventKind::StatusChanged { from, to }
    }

    fn edited(fields: &[&str]) -> EventKind {
        EventKind::Edited {
            fields: fields.iter().map(|f| f.to_string()).collect(),
        }
    }

    /// A unique database file in the temp dir, removed (with any journal)
    /// on drop, also when the test panics.
    struct TempDb(std::path::PathBuf);

    impl TempDb {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "fortnight-sqlite-{tag}-{}-{}.db",
                std::process::id(),
                Timestamp::now().as_nanosecond()
            ));
            let db = TempDb(path);
            db.remove();
            db
        }

        fn remove(&self) {
            let _ = std::fs::remove_file(&self.0);
            let mut journal = self.0.clone().into_os_string();
            journal.push("-journal");
            let _ = std::fs::remove_file(journal);
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            self.remove();
        }
    }

    #[test]
    fn migrates_round1_fixture_without_loss() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/round1.db");
        let tmp = TempDb::new("round1");
        std::fs::copy(&fixture, &tmp.0).unwrap();
        assert_eq!(user_version(&Connection::open(&tmp.0).unwrap()), 1);

        let s = SqliteStore::open(&tmp.0).unwrap();
        assert_eq!(user_version(&s.conn), 3);

        // Tasks: every value survives; the round 2 fields take defaults.
        let tasks = s.list_tasks().unwrap();
        let got: Vec<_> = tasks
            .iter()
            .map(|t| {
                (
                    t.id,
                    t.title.as_str(),
                    t.notes.as_str(),
                    t.project.as_deref(),
                    t.due_on,
                    t.status,
                    t.position,
                )
            })
            .collect();
        let d = |day| Some(date(2026, 11, day));
        assert_eq!(
            got,
            [
                (
                    1,
                    "Ship login page",
                    "Behind feature flag",
                    Some("website"),
                    d(13),
                    Done,
                    1.0
                ),
                (
                    2,
                    "Vendor contract review",
                    "",
                    Some("legal"),
                    d(12),
                    Todo,
                    2.0
                ),
                (
                    3,
                    "Write Q4 report",
                    "Draft in docs",
                    Some("work"),
                    d(20),
                    Todo,
                    3.0
                ),
                (4, "Plan offsite", "", None, None, Todo, 4.0),
                (
                    5,
                    "Migrate analytics",
                    "Edited notes",
                    Some("website"),
                    d(18),
                    Todo,
                    5.0
                ),
                (6, "Book dentist", "", None, d(16), Todo, 6.0),
            ]
        );
        for t in &tasks {
            // Migration 3: the deadline became the planned day.
            assert_eq!(t.planned_for, t.due_on);
            assert_eq!(t.estimate_minutes, None);
            assert_eq!(t.priority, 0);
            assert!(!t.private);
            assert_eq!(t.completed_at.is_some(), t.is_done());
        }
        assert_eq!(tasks[0].created_at, ts("2026-11-17T01:45:57.363926Z"));
        assert_eq!(
            tasks[0].completed_at,
            Some(ts("2026-11-17T01:45:57.365869Z"))
        );
        assert_eq!(tasks[4].updated_at, ts("2026-11-17T01:45:57.366093Z"));

        // Reports, including the user's edit and the failed attempt.
        let reports = s.all_reports().unwrap();
        assert_eq!(reports.len(), 3);
        let summary = s
            .latest_report(ReportKind::Summary, date(2026, 11, 9))
            .unwrap()
            .unwrap();
        assert_eq!(summary.status, ReportStatus::Ready);
        assert_eq!(summary.digest_json, r#"{"week":"2026-11-09"}"#);
        assert_eq!(
            summary.content_md.as_deref(),
            Some("## Highlights\n- Shipped the login page")
        );
        assert_eq!(
            summary.text(),
            Some("## Highlights\n- Shipped the login page (edited by me)")
        );
        assert_eq!(summary.model.as_deref(), Some("gpt-x"));
        assert_eq!(summary.endpoint_host.as_deref(), Some("localhost:9999"));
        assert_eq!(summary.prompt_version, 2);
        let plans = s
            .list_reports(ReportKind::Plan, date(2026, 11, 16))
            .unwrap();
        assert_eq!(plans.iter().map(|r| r.id).collect::<Vec<_>>(), [3, 2]);
        assert_eq!(plans[0].status, ReportStatus::Ready);
        assert_eq!(plans[1].status, ReportStatus::Failed);
        assert_eq!(plans[1].error.as_deref(), Some("could not connect"));
        assert_eq!(plans[1].content_md, None);
        assert_eq!(s.ready_report_keys().unwrap().len(), 2);

        // Settings: round 1 values kept, new fields default.
        let settings = s.load_settings().unwrap();
        assert_eq!(settings.llm.base_url, "http://127.0.0.1:9999/v1");
        assert_eq!(settings.llm.model, "gpt-x");
        assert_eq!(settings.llm.protocol, crate::domain::Protocol::OpenAi);
        assert_eq!(settings.schedule.working_days.len(), 4);
        assert_eq!(settings.summary_instructions, "Keep it under 150 words.");
        assert_eq!(
            settings.focus_minutes_per_day,
            crate::domain::DEFAULT_FOCUS_MINUTES
        );

        // Backfilled history: 6 created + 1 status_changed.
        let events = all_events(&s);
        assert_eq!(events.len(), 7);
        for t in &tasks {
            let ev = s.task_events(t.id).unwrap();
            assert_eq!(ev[0].kind, created(&t.title));
            assert_eq!(ev[0].at, t.created_at);
            if t.is_done() {
                assert_eq!(ev.len(), 2);
                assert_eq!(ev[1].kind, status(Todo, Done));
                assert_eq!(Some(ev[1].at), t.completed_at);
            } else {
                assert_eq!(ev.len(), 1);
            }
        }

        // The rebuilt table no longer reuses ids.
        let new = s.add_task(new_task("after migration")).unwrap();
        assert_eq!(new.id, 7);
        assert_eq!(new.position, 7.0);
        drop(s);

        // Reopening is a no-op.
        let s = SqliteStore::open(&tmp.0).unwrap();
        assert_eq!(user_version(&s.conn), 3);
        assert_eq!(&s.list_tasks().unwrap()[..6], &tasks[..]);
        assert_eq!(s.all_reports().unwrap(), reports);
        assert_eq!(s.load_settings().unwrap(), settings);
        assert_eq!(all_events(&s).len(), 8);
    }

    /// A v1 database as round 1 would have left it.
    fn v1_connection(conn: &Connection) {
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute_batch(
            r#"INSERT INTO tasks (id, title, notes, status, project, due_on, planned_for,
                                 estimate_minutes, priority, private, position,
                                 created_at, updated_at, completed_at) VALUES
               (3, 'open', 'n', 'todo', 'p', '2026-11-20', '2026-11-18', 90, 2, 1, 1.5,
                '2026-11-10T09:00:00.000000000Z', '2026-11-10T09:30:00.000000000Z', NULL),
               (8, 'finished "quoted" \ ü', '', 'done', NULL, NULL, NULL, NULL, 0, 0, 2.0,
                '2026-11-11T09:00:00.000000000Z', '2026-11-12T09:00:00.000000000Z',
                '2026-11-12T08:00:00.000000000Z');
               INSERT INTO reports (kind, week_start, status, digest_json, prompt_version,
                                    created_at)
               VALUES ('plan', '2026-11-16', 'ready', '{}', 1,
                       '2026-11-16T08:00:00.000000000Z');
               INSERT INTO settings (key, value) VALUES ('settings', '{"summary_instructions":"x"}');"#,
        )
        .unwrap();
    }

    #[test]
    fn migrates_in_memory_v1_schema() {
        let conn = Connection::open_in_memory().unwrap();
        v1_connection(&conn);
        let s = SqliteStore::init(conn).unwrap();
        assert_eq!(user_version(&s.conn), 3);

        let tasks = s.list_tasks().unwrap();
        assert_eq!(tasks.len(), 2);
        let open = &tasks[0];
        assert_eq!(open.id, 3);
        assert_eq!(open.planned_for, Some(date(2026, 11, 18)));
        assert_eq!(open.due_on, Some(date(2026, 11, 20)));
        assert_eq!(open.estimate_minutes, Some(90));
        assert_eq!(open.priority, 2);
        assert!(open.private);
        assert_eq!(open.position, 1.5);
        assert_eq!(open.updated_at, ts("2026-11-10T09:30:00Z"));

        let done = &tasks[1];
        assert_eq!(done.title, r#"finished "quoted" \ ü"#);
        assert_eq!(
            kinds(&s.task_events(done.id).unwrap()),
            [created(&done.title), status(Todo, Done)]
        );
        assert_eq!(
            s.task_events(done.id).unwrap()[1].at,
            ts("2026-11-12T08:00:00Z")
        );
        assert_eq!(kinds(&s.task_events(3).unwrap()), [created("open")]);

        // The SQL-built payloads are what serde would have written.
        let payloads: Vec<(String, String)> = s
            .conn
            .prepare("SELECT kind, payload FROM events ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let expected: Vec<(String, String)> =
            [created("open"), created(&done.title), status(Todo, Done)]
                .iter()
                .map(|k| (k.as_str().to_string(), serde_json::to_string(k).unwrap()))
                .collect();
        assert_eq!(payloads, expected);

        assert_eq!(s.all_reports().unwrap().len(), 1);
        assert_eq!(s.load_settings().unwrap().summary_instructions, "x");
        // Ids continue past the highest existing one.
        assert_eq!(s.add_task(new_task("next")).unwrap().id, 9);
    }

    #[test]
    fn failed_migration_changes_nothing() {
        let tmp = TempDb::new("failed-migration");
        {
            let conn = Connection::open(&tmp.0).unwrap();
            v1_connection(&conn);
            // Collides with an index migration 2 creates near its end, after
            // the task table rebuild.
            conn.execute_batch("CREATE INDEX events_task ON reports (kind);")
                .unwrap();
        }
        assert!(matches!(SqliteStore::open(&tmp.0), Err(StoreError::Db(_))));

        let conn = Connection::open(&tmp.0).unwrap();
        assert_eq!(user_version(&conn), 1);
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(tables, ["reports", "settings", "tasks"]);
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'tasks'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!sql.contains("AUTOINCREMENT"));
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn add_task_saves_new_fields_and_writes_created() {
        let s = store();
        let t = s
            .add_task(NewTask {
                title: "deep work".into(),
                planned_for: Some(date(2026, 11, 18)),
                estimate_minutes: Some(120),
                priority: 3,
                private: true,
                ..NewTask::default()
            })
            .unwrap();
        assert_eq!(t.planned_for, Some(date(2026, 11, 18)));
        assert_eq!(t.estimate_minutes, Some(120));
        assert_eq!(t.priority, 3);
        assert!(t.private);
        assert_eq!(s.get_task(t.id).unwrap(), Some(t.clone()));

        let events = s.task_events(t.id).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].task_id, Some(t.id));
        assert_eq!(events[0].at, t.created_at);
        assert_eq!(events[0].kind, created("deep work"));
        let (kind, payload): (String, String) = s
            .conn
            .query_row("SELECT kind, payload FROM events", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(kind, "created");
        assert_eq!(payload, r#"{"kind":"created","title":"deep work"}"#);
    }

    #[test]
    fn update_task_events_follow_the_changes() {
        let s = store();
        let t = s.add_task(new_task("t")).unwrap();
        let count = |s: &SqliteStore| s.task_events(t.id).unwrap().len();

        // Unchanged: nothing written, updated_at kept. Status and position
        // are not editable here, so changing them is also "unchanged".
        let mut same = t.clone();
        same.status = Done;
        same.position = 99.0;
        s.update_task(&same).unwrap();
        assert_eq!(s.get_task(t.id).unwrap(), Some(t.clone()));
        assert_eq!(count(&s), 1);

        // Deadline only.
        let mut e = t.clone();
        e.due_on = Some(date(2026, 11, 20));
        s.update_task(&e).unwrap();
        let ev = s.task_events(t.id).unwrap();
        assert_eq!(
            ev.last().unwrap().kind,
            EventKind::DueChanged {
                from: None,
                to: Some(date(2026, 11, 20))
            }
        );
        let got = s.get_task(t.id).unwrap().unwrap();
        assert_eq!(ev.last().unwrap().at, got.updated_at);
        assert!(got.updated_at >= t.updated_at);

        // Planned day only.
        e.planned_for = Some(date(2026, 11, 18));
        s.update_task(&e).unwrap();
        assert_eq!(
            s.task_events(t.id).unwrap().last().unwrap().kind,
            EventKind::PlannedChanged {
                from: None,
                to: Some(date(2026, 11, 18))
            }
        );
        assert_eq!(count(&s), 3);

        // Other fields: one Edited naming them.
        e.title = "t2".into();
        e.estimate_minutes = Some(30);
        e.private = true;
        s.update_task(&e).unwrap();
        assert_eq!(
            s.task_events(t.id).unwrap().last().unwrap().kind,
            edited(&["title", "estimate", "private"])
        );
        assert_eq!(count(&s), 4);

        // Everything at once: three events at the same moment.
        e.due_on = None;
        e.planned_for = Some(date(2026, 11, 19));
        e.notes = "n".into();
        e.project = Some("p".into());
        e.priority = 1;
        s.update_task(&e).unwrap();
        let got = s.get_task(t.id).unwrap().unwrap();
        let ev = s.task_events(t.id).unwrap();
        assert_eq!(
            kinds(&ev[4..]),
            [
                EventKind::DueChanged {
                    from: Some(date(2026, 11, 20)),
                    to: None
                },
                EventKind::PlannedChanged {
                    from: Some(date(2026, 11, 18)),
                    to: Some(date(2026, 11, 19))
                },
                edited(&["notes", "project", "priority"]),
            ]
        );
        assert!(ev[4..].iter().all(|x| x.at == got.updated_at));
        assert_eq!(
            (
                got.title.as_str(),
                got.notes.as_str(),
                got.project.as_deref()
            ),
            ("t2", "n", Some("p"))
        );
        assert_eq!(
            (got.estimate_minutes, got.priority, got.private),
            (Some(30), 1, true)
        );
        assert_eq!(got.status, Todo);
        assert_eq!(got.position, t.position);

        // Saving the stored task again changes nothing.
        s.update_task(&got).unwrap();
        assert_eq!(s.get_task(t.id).unwrap(), Some(got));
        assert_eq!(count(&s), 7);
    }

    #[test]
    fn set_position_changes_only_the_order() {
        let s = store();
        let a = s.add_task(new_task("a")).unwrap();
        let b = s.add_task(new_task("b")).unwrap();
        s.set_position(b.id, 0.5).unwrap();
        let got = s.list_tasks().unwrap();
        assert_eq!(got[0].id, b.id);
        assert_eq!(got[0].updated_at, b.updated_at);
        assert_eq!(s.task_events(b.id).unwrap().len(), 1);
        assert!(s.set_position(999, 1.0).is_err());
        assert!(s.set_position(a.id, f64::NAN).is_err());
    }

    #[test]
    fn set_status_events_and_completed_at() {
        let s = store();
        let t = s.add_task(new_task("t")).unwrap();

        s.set_status(t.id, Doing).unwrap();
        let got = s.get_task(t.id).unwrap().unwrap();
        assert_eq!((got.status, got.completed_at), (Doing, None));

        s.set_status(t.id, Done).unwrap();
        let done = s.get_task(t.id).unwrap().unwrap();
        assert_eq!(done.status, Done);
        assert_eq!(done.completed_at, Some(done.updated_at));

        // Already done: nothing changes.
        s.set_status(t.id, Done).unwrap();
        assert_eq!(s.get_task(t.id).unwrap(), Some(done.clone()));

        s.set_status(t.id, Dropped).unwrap();
        let dropped = s.get_task(t.id).unwrap().unwrap();
        assert_eq!((dropped.status, dropped.completed_at), (Dropped, None));

        s.set_status(t.id, Todo).unwrap();
        let ev = s.task_events(t.id).unwrap();
        assert_eq!(
            kinds(&ev),
            [
                created("t"),
                status(Todo, Doing),
                status(Doing, Done),
                status(Done, Dropped),
                status(Dropped, Todo),
            ]
        );
        assert_eq!(Some(ev[2].at), done.completed_at);
        assert_eq!(ev[3].at, dropped.updated_at);
    }

    #[test]
    fn delete_then_restore_round_trips() {
        let s = store();
        let keep = s.add_task(new_task("keep")).unwrap();
        let t = s
            .add_task(NewTask {
                title: "t".into(),
                notes: "n".into(),
                project: Some("p".into()),
                due_on: Some(date(2026, 11, 20)),
                planned_for: Some(date(2026, 11, 19)),
                estimate_minutes: Some(45),
                priority: 2,
                private: true,
            })
            .unwrap();
        let mut e = t.clone();
        e.due_on = Some(date(2026, 11, 27));
        s.update_task(&e).unwrap();
        s.set_status(t.id, Done).unwrap();
        s.add_log(Some(t.id), "worked on it").unwrap();
        let free = s.add_log(None, "free-standing").unwrap();

        let task_before = s.get_task(t.id).unwrap().unwrap();
        let events_before = s.task_events(t.id).unwrap();
        let all_before = all_events(&s);
        let list_before = s.list_tasks().unwrap();
        assert_eq!(events_before.len(), 4);

        let deleted = s.delete_task(t.id).unwrap();
        assert_eq!(deleted.task, task_before);
        assert_eq!(deleted.events, events_before);
        assert_eq!(s.get_task(t.id).unwrap(), None);
        assert!(s.task_events(t.id).unwrap().is_empty());
        let all_after = all_events(&s);
        assert_eq!(all_after.len(), all_before.len() - 4);
        assert!(all_after.contains(&free));
        assert_eq!(s.list_tasks().unwrap(), std::slice::from_ref(&keep));

        // New activity between delete and undo does not take the ids.
        let other = s.add_task(new_task("other")).unwrap();
        assert!(other.id > t.id);
        let log = s.add_log(None, "later").unwrap();
        assert!(events_before.iter().all(|x| x.id < log.id));

        s.restore_task(&deleted).unwrap();
        assert_eq!(s.get_task(t.id).unwrap(), Some(task_before));
        assert_eq!(s.task_events(t.id).unwrap(), events_before);
        let mut expected = all_before.clone();
        expected.extend(s.task_events(other.id).unwrap());
        expected.push(log);
        expected.sort_by_key(|x| (x.at, x.id));
        assert_eq!(all_events(&s), expected);
        let mut list = list_before;
        list.push(other);
        assert_eq!(s.list_tasks().unwrap(), list);

        // Restoring twice fails and adds nothing.
        assert!(matches!(s.restore_task(&deleted), Err(StoreError::Db(_))));
        assert_eq!(all_events(&s), expected);
    }

    #[test]
    fn a_log_line_can_be_dated() {
        let s = store();
        let later: Timestamp = "2030-01-02T12:00:00Z".parse().unwrap();
        let dated = s.add_log_at(None, "a future day's notes", later).unwrap();
        assert_eq!(dated.at, later);
        assert_eq!(
            s.events_between(later, Timestamp::MAX).unwrap(),
            vec![dated]
        );
        assert_eq!(
            s.add_log_at(Some(42), "x", later),
            Err(StoreError::TaskNotFound(42))
        );
    }

    #[test]
    fn log_lines_attached_and_free_standing() {
        let s = store();
        let t = s.add_task(new_task("t")).unwrap();
        let created_event = s.task_events(t.id).unwrap().remove(0);

        let attached = s.add_log(Some(t.id), "paired with Sam").unwrap();
        assert_eq!(attached.task_id, Some(t.id));
        assert_eq!(attached.log_text(), Some("paired with Sam"));
        assert!(attached.at >= t.created_at);
        let free = s.add_log(None, "inbox zero").unwrap();
        assert_eq!(free.task_id, None);
        assert_eq!(
            free.kind,
            EventKind::Log {
                text: "inbox zero".into()
            }
        );

        // What add_log returns is what reads give back.
        assert_eq!(
            s.task_events(t.id).unwrap(),
            [created_event.clone(), attached.clone()]
        );
        assert_eq!(
            all_events(&s),
            [created_event.clone(), attached.clone(), free.clone()]
        );

        assert_eq!(
            s.add_log(Some(999), "x"),
            Err(StoreError::TaskNotFound(999))
        );
        assert_eq!(all_events(&s).len(), 3);

        // Only log lines can be deleted.
        assert_eq!(
            s.delete_log(created_event.id),
            Err(StoreError::EventNotFound(created_event.id))
        );
        assert_eq!(s.delete_log(12345), Err(StoreError::EventNotFound(12345)));
        s.delete_log(attached.id).unwrap();
        assert_eq!(
            s.delete_log(attached.id),
            Err(StoreError::EventNotFound(attached.id))
        );
        s.delete_log(free.id).unwrap();
        assert_eq!(all_events(&s), [created_event]);
    }

    #[test]
    fn events_between_bounds() {
        let s = store();
        let t = s.add_task(new_task("t")).unwrap();
        let ids: Vec<EventId> = [
            s.task_events(t.id).unwrap()[0].id,
            s.add_log(Some(t.id), "a").unwrap().id,
            s.add_log(None, "b").unwrap().id,
            s.add_log(None, "c").unwrap().id,
            s.add_log(None, "d").unwrap().id,
        ]
        .into();
        let t0 = ts("2026-11-09T00:00:00Z");
        let t1 = ts("2026-11-10T12:00:00Z");
        let t2 = ts("2026-11-16T00:00:00Z");
        let t3 = ts("2026-11-16T00:00:00.000000001Z");
        // Events 3 and 4 share a moment, so `id` breaks the tie. Stored out
        // of order to show the sort is by `at`, not insertion.
        for (id, at) in ids.iter().zip([t2, t0, t1, t3, t3]) {
            s.conn
                .execute(
                    "UPDATE events SET at = ?2 WHERE id = ?1",
                    params![id, ts_to_sql(at)],
                )
                .unwrap();
        }
        let between = |from, to| -> Vec<EventId> {
            s.events_between(from, to)
                .unwrap()
                .iter()
                .map(|e| e.id)
                .collect()
        };
        let [e_t2, e_t0, e_t1, e_t3a, e_t3b] = ids[..] else {
            unreachable!()
        };
        assert_eq!(between(t0, t2), [e_t0, e_t1], "to is exclusive");
        assert_eq!(between(t2, t3), [e_t2], "from is inclusive");
        assert_eq!(between(t1, t1), Vec::<EventId>::new());
        assert_eq!(between(t2, t0), Vec::<EventId>::new());
        assert_eq!(
            between(Timestamp::MIN, Timestamp::MAX),
            [e_t0, e_t1, e_t2, e_t3a, e_t3b]
        );
        assert_eq!(between(Timestamp::MIN, t1), [e_t0]);
        assert_eq!(between(t1, Timestamp::MAX), [e_t1, e_t2, e_t3a, e_t3b]);
        assert_eq!(
            between(Timestamp::MIN, Timestamp::MIN),
            Vec::<EventId>::new()
        );
        assert_eq!(
            between(Timestamp::MAX, Timestamp::MAX),
            Vec::<EventId>::new()
        );
        let far_past = ts("-000001-06-01T00:00:00Z");
        assert_eq!(between(far_past, t1), [e_t0]);
        assert_eq!(
            between(far_past, ts("0001-01-01T00:00:00Z")),
            Vec::<EventId>::new()
        );
    }

    #[test]
    fn failures_leave_no_partial_writes() {
        let s = store();
        let t = s.add_task(new_task("t")).unwrap();
        let block_events = "CREATE TEMP TRIGGER block BEFORE INSERT ON events
                            BEGIN SELECT RAISE(ABORT, 'blocked'); END;";
        s.conn.execute_batch(block_events).unwrap();

        assert!(matches!(s.add_task(new_task("u")), Err(StoreError::Db(_))));
        let mut e = t.clone();
        e.title = "changed".into();
        e.due_on = Some(date(2026, 11, 20));
        assert!(s.update_task(&e).is_err());
        assert!(s.set_status(t.id, Done).is_err());
        assert!(s.add_log(Some(t.id), "x").is_err());
        assert!(s.add_log(None, "x").is_err());
        assert_eq!(s.list_tasks().unwrap(), std::slice::from_ref(&t));
        assert_eq!(all_events(&s).len(), 1);

        // A restore whose events fail leaves no task behind.
        s.conn.execute_batch("DROP TRIGGER block").unwrap();
        let deleted = s.delete_task(t.id).unwrap();
        s.conn.execute_batch(block_events).unwrap();
        assert!(s.restore_task(&deleted).is_err());
        assert_eq!(s.get_task(t.id).unwrap(), None);

        // A delete whose task row fails keeps the events.
        s.conn.execute_batch("DROP TRIGGER block").unwrap();
        s.restore_task(&deleted).unwrap();
        s.conn
            .execute_batch(
                "CREATE TEMP TRIGGER keep BEFORE DELETE ON tasks
                 BEGIN SELECT RAISE(ABORT, 'kept'); END;",
            )
            .unwrap();
        assert!(s.delete_task(t.id).is_err());
        assert_eq!(s.task_events(t.id).unwrap(), deleted.events);
        assert_eq!(s.get_task(t.id).unwrap(), Some(t));
    }

    #[test]
    fn corrupt_event_is_invalid() {
        let s = store();
        let t = s.add_task(new_task("t")).unwrap();
        s.conn
            .execute("UPDATE events SET payload = '{\"kind\":\"nope\"}'", [])
            .unwrap();
        assert!(matches!(s.task_events(t.id), Err(StoreError::Invalid(_))));
        s.conn
            .execute(
                "UPDATE events SET payload = '{\"kind\":\"log\",\"text\":\"x\"}'",
                [],
            )
            .unwrap();
        // `kind` column says created, payload says log.
        assert!(matches!(s.task_events(t.id), Err(StoreError::Invalid(_))));
    }

    #[test]
    fn list_and_all_reports_order() {
        let s = store();
        let w1 = date(2026, 11, 9);
        let w2 = date(2026, 11, 16);
        use ReportKind::*;
        use ReportStatus::*;
        let a = s
            .insert_report(new_report(Summary, w2, Ready, "a"))
            .unwrap();
        let b = s
            .insert_report(new_report(Summary, w2, Failed, "b"))
            .unwrap();
        let c = s.insert_report(new_report(Plan, w2, Ready, "c")).unwrap();
        let d = s
            .insert_report(new_report(Summary, w1, Ready, "d"))
            .unwrap();
        let e = s
            .insert_report(new_report(Summary, w2, Ready, "e"))
            .unwrap();

        let ids = |rs: Vec<Report>| rs.into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(
            ids(s.list_reports(Summary, w2).unwrap()),
            [e.id, b.id, a.id]
        );
        assert_eq!(ids(s.list_reports(Summary, w1).unwrap()), [d.id]);
        assert_eq!(ids(s.list_reports(Plan, w1).unwrap()), Vec::<i64>::new());
        assert_eq!(
            ids(s.all_reports().unwrap()),
            [d.id, c.id, a.id, b.id, e.id]
        );

        // Ties on created_at: newest id first in list_reports.
        s.conn
            .execute(
                "UPDATE reports SET created_at = ?1",
                [ts_to_sql(a.created_at)],
            )
            .unwrap();
        assert_eq!(
            ids(s.list_reports(Summary, w2).unwrap()),
            [e.id, b.id, a.id]
        );
        assert_eq!(
            s.list_reports(Summary, w2).unwrap()[0],
            s.latest_report(Summary, w2).unwrap().unwrap()
        );
    }
}
