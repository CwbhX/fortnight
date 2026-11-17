//! Domain types shared by every module. FROZEN after step 1: do not edit.
//!
//! Conventions:
//! - Deadlines are civil dates (`jiff::civil::Date`), stored as `YYYY-MM-DD`.
//! - Moments are UTC timestamps (`jiff::Timestamp`).
//! - Nothing here depends on the machine's time zone.

use jiff::civil::{Date, Time, Weekday};
use jiff::{Timestamp, ToSpan};
use serde::{Deserialize, Serialize};

pub type TaskId = i64;
pub type ReportId = i64;

/// Task status. The prototype only uses `Todo` and `Done`; the other two
/// exist so the schema matches PLAN.md section 6 without a later migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Todo,
    Doing,
    Done,
    Dropped,
}

impl TaskStatus {
    /// The string stored in the `tasks.status` column.
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Todo => "todo",
            TaskStatus::Doing => "doing",
            TaskStatus::Done => "done",
            TaskStatus::Dropped => "dropped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "todo" => Some(TaskStatus::Todo),
            "doing" => Some(TaskStatus::Doing),
            "done" => Some(TaskStatus::Done),
            "dropped" => Some(TaskStatus::Dropped),
            _ => None,
        }
    }

    /// Open means still to be done: `Todo` or `Doing`.
    pub fn is_open(self) -> bool {
        matches!(self, TaskStatus::Todo | TaskStatus::Doing)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub notes: String,
    pub status: TaskStatus,
    pub project: Option<String>,
    pub due_on: Option<Date>,
    /// Manual ordering; lower sorts first.
    pub position: f64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Set when the task becomes `Done`, cleared when it is reopened.
    pub completed_at: Option<Timestamp>,
}

impl Task {
    pub fn is_done(&self) -> bool {
        self.status == TaskStatus::Done
    }
}

/// What the store needs to create a task. Status starts as `Todo`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NewTask {
    pub title: String,
    pub notes: String,
    pub project: Option<String>,
    pub due_on: Option<Date>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportKind {
    /// The weekly summary of the week behind you.
    Summary,
    /// Advice for the week ahead.
    Plan,
}

impl ReportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ReportKind::Summary => "summary",
            ReportKind::Plan => "plan",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "summary" => Some(ReportKind::Summary),
            "plan" => Some(ReportKind::Plan),
            _ => None,
        }
    }
}

/// The prototype only writes `Ready` and `Failed` rows. "Generating" is
/// in-memory state in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportStatus {
    Ready,
    Failed,
}

impl ReportStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ReportStatus::Ready => "ready",
            ReportStatus::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ready" => Some(ReportStatus::Ready),
            "failed" => Some(ReportStatus::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub id: ReportId,
    pub kind: ReportKind,
    /// Civil date of the week's first day.
    pub week_start: Date,
    pub status: ReportStatus,
    /// The exact facts given to the model, as JSON.
    pub digest_json: String,
    /// Model output.
    pub content_md: Option<String>,
    /// The user's edits; shown in preference to `content_md`.
    pub edited_md: Option<String>,
    pub model: Option<String>,
    pub endpoint_host: Option<String>,
    pub prompt_version: i64,
    pub error: Option<String>,
    pub created_at: Timestamp,
}

impl Report {
    /// The text to show: the user's edit if any, otherwise the model output.
    pub fn text(&self) -> Option<&str> {
        self.edited_md.as_deref().or(self.content_md.as_deref())
    }

    pub fn key(&self) -> ReportKey {
        ReportKey {
            kind: self.kind,
            week_start: self.week_start,
        }
    }
}

/// What the store needs to insert a report. `created_at` is set by the store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewReport {
    pub kind: ReportKind,
    pub week_start: Date,
    pub status: ReportStatus,
    pub digest_json: String,
    pub content_md: Option<String>,
    pub model: Option<String>,
    pub endpoint_host: Option<String>,
    pub prompt_version: i64,
    pub error: Option<String>,
}

/// Identifies "the report of this kind for this week".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReportKey {
    pub kind: ReportKind,
    pub week_start: Date,
}

/// A week, identified by its first day. Which weekday a week starts on is a
/// setting (`Schedule::week_start`); `week::week_containing` builds one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WeekRef {
    pub start: Date,
}

impl WeekRef {
    pub fn new(start: Date) -> Self {
        WeekRef { start }
    }

    /// The last day of the week (start + 6 days).
    pub fn end(self) -> Date {
        self.start.saturating_add(6.days())
    }

    /// The seven days of the week, in order.
    pub fn days(self) -> [Date; 7] {
        let mut days = [self.start; 7];
        for (i, day) in days.iter_mut().enumerate() {
            *day = self.start.saturating_add((i as i64).days());
        }
        days
    }

    pub fn contains(self, date: Date) -> bool {
        date >= self.start && date <= self.end()
    }

    pub fn prev(self) -> Self {
        WeekRef::new(self.start.saturating_sub(7.days()))
    }

    pub fn next(self) -> Self {
        WeekRef::new(self.start.saturating_add(7.days()))
    }
}

/// LLM endpoint settings. The API key is not here: it lives encrypted in the
/// key file (see `vault`) and in memory once unlocked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmSettings {
    /// e.g. `https://api.openai.com/v1` or `http://localhost:11434/v1`.
    /// The client appends `/chat/completions`.
    pub base_url: String,
    pub model: String,
}

impl LlmSettings {
    pub fn is_configured(&self) -> bool {
        !self.base_url.trim().is_empty() && !self.model.trim().is_empty()
    }
}

/// When reports are due. Times are local wall-clock times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Schedule {
    #[serde(with = "weekday_serde")]
    pub week_start: Weekday,
    #[serde(with = "weekday_serde")]
    pub summary_day: Weekday,
    pub summary_time: Time,
    #[serde(with = "weekday_serde")]
    pub plan_day: Weekday,
    pub plan_time: Time,
    /// Working days, used by the plan facts.
    #[serde(with = "weekdays_serde")]
    pub working_days: Vec<Weekday>,
}

/// jiff 1.0 has no serde support for `Weekday`; store it as 1 (Monday)
/// to 7 (Sunday).
mod weekday_serde {
    use jiff::civil::Weekday;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(day: &Weekday, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i8(day.to_monday_one_offset())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Weekday, D::Error> {
        let n = i8::deserialize(d)?;
        Weekday::from_monday_one_offset(n).map_err(serde::de::Error::custom)
    }
}

mod weekdays_serde {
    use jiff::civil::Weekday;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(days: &[Weekday], s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(days.iter().map(|d| d.to_monday_one_offset()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Weekday>, D::Error> {
        Vec::<i8>::deserialize(d)?
            .into_iter()
            .map(|n| Weekday::from_monday_one_offset(n).map_err(serde::de::Error::custom))
            .collect()
    }
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule {
            week_start: Weekday::Monday,
            summary_day: Weekday::Friday,
            summary_time: Time::constant(16, 0, 0, 0),
            plan_day: Weekday::Monday,
            plan_time: Time::constant(8, 0, 0, 0),
            working_days: vec![
                Weekday::Monday,
                Weekday::Tuesday,
                Weekday::Wednesday,
                Weekday::Thursday,
                Weekday::Friday,
            ],
        }
    }
}

/// All user settings. Stored in the `settings` table so they travel with
/// the data folder. Missing fields fall back to defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub llm: LlmSettings,
    pub schedule: Schedule,
    /// Free-form extra instructions for the summary (length, tone, ...).
    pub summary_instructions: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_json() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), s);
        assert_eq!(serde_json::from_str::<Settings>("{}").unwrap(), s);
    }

    #[test]
    fn week_ref_days() {
        let w = WeekRef::new(jiff::civil::date(2026, 11, 16));
        assert_eq!(w.end(), jiff::civil::date(2026, 11, 22));
        assert_eq!(w.days()[6], w.end());
        assert!(w.contains(jiff::civil::date(2026, 11, 22)));
        assert!(!w.contains(jiff::civil::date(2026, 11, 23)));
        assert_eq!(w.next().prev(), w);
    }
}
