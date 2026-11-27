//! Domain types shared by every module. FROZEN (round 2): do not edit.
//! (Round 3, at the user's request: `Settings` gained the two prompts, and
//! `Task::day` became the planned day only.)
//!
//! Conventions:
//! - Deadlines are civil dates (`jiff::civil::Date`), stored as `YYYY-MM-DD`.
//! - Moments are UTC timestamps (`jiff::Timestamp`).
//! - Nothing here depends on the machine's time zone.

use jiff::civil::{Date, Weekday};
use jiff::{Timestamp, ToSpan};
use serde::{Deserialize, Serialize};

pub type TaskId = i64;
pub type ReportId = i64;
pub type EventId = i64;

/// Task status. `Doing` is "in progress"; `Dropped` is "won't do" (kept for
/// the history, shown dimmed like `Done`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    #[default]
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

/// Priority: 0 is normal; 1 to 3 are increasingly high (`!`, `!!`, `!!!`
/// in the quick-add bar).
pub type Priority = u8;

/// The highest priority the app uses.
pub const MAX_PRIORITY: Priority = 3;

/// How often a task repeats when marked done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Recurrence {
    Daily,
    Weekdays,
    Weekly,
    Monthly,
}

impl Recurrence {
    /// Every choice, in the order pickers show them.
    pub const ALL: [Recurrence; 4] = [
        Recurrence::Daily,
        Recurrence::Weekdays,
        Recurrence::Weekly,
        Recurrence::Monthly,
    ];

    /// The string stored in the `tasks.recurrence` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Recurrence::Daily => "daily",
            Recurrence::Weekdays => "weekdays",
            Recurrence::Weekly => "weekly",
            Recurrence::Monthly => "monthly",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "daily" | "day" => Some(Recurrence::Daily),
            "weekdays" | "weekday" => Some(Recurrence::Weekdays),
            "weekly" | "week" => Some(Recurrence::Weekly),
            "monthly" | "month" => Some(Recurrence::Monthly),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Recurrence::Daily => "Daily",
            Recurrence::Weekdays => "Weekdays",
            Recurrence::Weekly => "Weekly",
            Recurrence::Monthly => "Monthly",
        }
    }

    /// The next date after `from` for this cadence.
    pub fn next_date(self, from: Date) -> Date {
        match self {
            Recurrence::Daily => from.saturating_add(1.days()),
            Recurrence::Weekdays => {
                let mut d = from.saturating_add(1.days());
                while matches!(d.weekday(), Weekday::Saturday | Weekday::Sunday) {
                    d = d.saturating_add(1.days());
                }
                d
            }
            Recurrence::Weekly => from.saturating_add(7.days()),
            Recurrence::Monthly => crate::week::add_months(from, 1),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub notes: String,
    pub status: TaskStatus,
    pub project: Option<String>,
    /// The deadline. It never decides where the task is shown.
    pub due_on: Option<Date>,
    /// The day you intend to do it: the day it is shown on. Set by quick
    /// add and by dragging the task to a day.
    pub planned_for: Option<Date>,
    /// Estimated effort in minutes (`~2h` → 120).
    pub estimate_minutes: Option<u32>,
    pub priority: Priority,
    /// Never sent to the LLM: excluded from every digest.
    pub private: bool,
    /// Repeat cadence: when marked done, spawns the next occurrence.
    #[serde(default)]
    pub recurrence: Option<Recurrence>,
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

    /// The day the task is shown on in the Today, Week and Month tabs: its
    /// planned day. The deadline does not move it. `None` means "No date"
    /// (the inbox). Every view and the load maths use this one rule.
    /// (Schema migration 3 copied the deadline into the planned day where
    /// that was empty, so older tasks stay where they were.)
    pub fn day(&self) -> Option<Date> {
        self.planned_for
    }

    /// Open and its deadline is before `today`.
    pub fn is_overdue(&self, today: Date) -> bool {
        self.status.is_open() && self.due_on.is_some_and(|d| d < today)
    }

    /// On today's list: open and planned for `today`, carried over from an
    /// earlier day, or with no planned day (e.g. added from the CLI without
    /// a date). The one rule for "today's list"; the Today tab, CLI
    /// `list --today`, report facts' `on_today_list`, and recommendations
    /// all use it.
    ///
    /// Overdue is a separate test (`is_overdue`): an overdue task planned
    /// for a later day is not on today's list by its deadline alone, so
    /// "today's list or overdue" is `on_today_list(t) || is_overdue(t)`.
    /// Done and deleted tasks are never on it.
    pub fn on_today_list(&self, today: Date) -> bool {
        self.status.is_open() && self.day().is_none_or(|d| d <= today)
    }
}

/// What the store needs to create a task. Status starts as `Todo`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NewTask {
    pub title: String,
    pub notes: String,
    pub project: Option<String>,
    pub due_on: Option<Date>,
    pub planned_for: Option<Date>,
    pub estimate_minutes: Option<u32>,
    pub priority: Priority,
    pub private: bool,
    #[serde(default)]
    pub recurrence: Option<Recurrence>,
}

/// Builds the next occurrence of a recurring task completed on `today`, or
/// `None` if the task does not repeat.
///
/// The next planned day advances from `max(planned_for, today)` so completing
/// a carried-over task schedules a future date. If a deadline was set too, its
/// offset from `planned_for` is preserved (or it advances directly if there was
/// no planned day). Checklist subtasks in `notes` are reset to unchecked.
pub fn next_occurrence(task: &Task, today: Date) -> Option<NewTask> {
    let recurrence = task.recurrence?;
    let base_planned = task.planned_for.unwrap_or(today).max(today);
    let next_planned = recurrence.next_date(base_planned);
    let next_due = match (task.planned_for, task.due_on) {
        (Some(p), Some(d)) => {
            let offset_days = p.until(d).map(|s| i64::from(s.get_days())).unwrap_or(0);
            Some(next_planned.saturating_add(offset_days.days()))
        }
        (None, Some(d)) => Some(recurrence.next_date(d.max(today))),
        (_, None) => None,
    };
    let (mut subtasks, text) = crate::notes::split_notes(&task.notes);
    let notes = if subtasks.is_empty() {
        task.notes.clone()
    } else {
        for s in &mut subtasks {
            s.done = false;
        }
        crate::notes::join_notes(&subtasks, &text)
    };
    Some(NewTask {
        title: task.title.clone(),
        notes,
        project: task.project.clone(),
        due_on: next_due,
        planned_for: Some(next_planned),
        estimate_minutes: task.estimate_minutes,
        priority: task.priority,
        private: task.private,
        recurrence: Some(recurrence),
    })
}

// ------------------------------------------------------------------ history

/// What happened, with its data. Stored as `events.kind` (`as_str`) plus
/// `events.payload` (this value as JSON, internally tagged by `kind`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    /// The task was created.
    Created {
        title: String,
    },
    StatusChanged {
        from: TaskStatus,
        to: TaskStatus,
    },
    /// The deadline moved (or was set or cleared).
    DueChanged {
        from: Option<Date>,
        to: Option<Date>,
    },
    PlannedChanged {
        from: Option<Date>,
        to: Option<Date>,
    },
    /// Other fields changed. `fields` names them, from: `title`, `notes`,
    /// `project`, `estimate`, `priority`, `private`.
    Edited {
        fields: Vec<String>,
    },
    /// A log line: work that isn't a checkbox. Attached to a task, or
    /// free-standing (`Event::task_id` is `None`).
    Log {
        text: String,
    },
}

impl EventKind {
    /// The string stored in the `events.kind` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::Created { .. } => "created",
            EventKind::StatusChanged { .. } => "status_changed",
            EventKind::DueChanged { .. } => "due_changed",
            EventKind::PlannedChanged { .. } => "planned_changed",
            EventKind::Edited { .. } => "edited",
            EventKind::Log { .. } => "log",
        }
    }

    pub fn is_log(&self) -> bool {
        matches!(self, EventKind::Log { .. })
    }
}

/// One entry in the append-only history (PLAN.md section 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: EventId,
    /// `None` for free-standing log lines.
    pub task_id: Option<TaskId>,
    pub at: Timestamp,
    pub kind: EventKind,
}

impl Event {
    /// The text of a log line; `None` for other kinds.
    pub fn log_text(&self) -> Option<&str> {
        match &self.kind {
            EventKind::Log { text } => Some(text),
            _ => None,
        }
    }
}

/// A deleted task and its history, as returned by `Store::delete_task`, so
/// that undo can put both back exactly (same ids).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeletedTask {
    pub task: Task,
    pub events: Vec<Event>,
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

/// The wire format of the endpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// OpenAI-compatible Chat Completions: `POST {base_url}/chat/completions`.
    #[default]
    OpenAi,
    /// Anthropic Messages: `POST {base_url}/v1/messages`.
    Anthropic,
    /// An Ollama decision model (Jev-style, Ollama 0.35+):
    /// `POST {root}/v1/systemone`, which answers questions with choices and
    /// probabilities, not text. Only recommended tasks can use it (see
    /// `decision`). `base_url` is the same as for Ollama's OpenAI endpoint,
    /// e.g. `http://localhost:11434/v1`.
    #[serde(rename = "ollama_decision")]
    OllamaDecision,
}

impl Protocol {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai" => Some(Protocol::OpenAi),
            "anthropic" => Some(Protocol::Anthropic),
            "ollama_decision" | "ollama-decision" | "decision" => Some(Protocol::OllamaDecision),
            _ => None,
        }
    }
}

/// LLM endpoint settings. The API key is not here: it lives encrypted in the
/// key file (see `vault`) and in memory once unlocked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmSettings {
    /// Missing in round 1 data, which therefore loads as `OpenAi`.
    pub protocol: Protocol,
    /// e.g. `https://api.openai.com/v1` or `http://localhost:11434/v1`.
    /// The OpenAI client appends `/chat/completions`; the Anthropic client
    /// appends `/v1/messages` (tolerating a base URL that already ends in
    /// `/v1`).
    pub base_url: String,
    pub model: String,
    /// The endpoint needs no API key (Ollama, LM Studio, a self-hosted
    /// server): no key is sent to it and none has to be unlocked for it.
    /// Missing in older data, which therefore loads as `false`.
    pub keyless: bool,
}

impl LlmSettings {
    pub fn is_configured(&self) -> bool {
        !self.base_url.trim().is_empty() && !self.model.trim().is_empty()
    }
}

/// The shape of the week: its first day and the working days. When the
/// background summary is due follows from these (`week::due_reports`).
/// Older data also holds `summary_day`, `summary_time`, `plan_day` and
/// `plan_time`; they are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Schedule {
    #[serde(with = "weekday_serde")]
    pub week_start: Weekday,
    /// Working days: the plan facts, the capacity, and the summary's due
    /// day (the last of them).
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
            week_start: Weekday::Sunday,
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
/// the data folder. Missing fields fall back to defaults, so round 1
/// settings load unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub llm: LlmSettings,
    /// Optional "decision" model for recommended tasks only, such as a
    /// reasoning model on a local Ollama. Empty (not configured) means
    /// recommended tasks use `llm`; so does any failure to reach it before
    /// it starts answering (see `fallback::FallbackClient`). It gets the API
    /// key only when it is on the same host as `llm`.
    pub decision_llm: LlmSettings,
    pub schedule: Schedule,
    /// Free-form extra instructions for the summary (length, tone, ...).
    pub summary_instructions: String,
    /// Focus time available on each working day, in minutes (shown as
    /// hours). The capacity the load bars and plan facts compare against.
    pub focus_minutes_per_day: u32,
    /// The system prompt for the work summary. Empty means the built-in
    /// one (`report::DEFAULT_SUMMARY_PROMPT`).
    pub summary_prompt: String,
    /// The system prompt for recommended tasks. Empty means the built-in
    /// one (`report::DEFAULT_PLAN_PROMPT`).
    pub plan_prompt: String,
    /// Light or dark colours, or follow the system. Missing in older data:
    /// follow the system.
    pub appearance: Appearance,
    /// Which app icon to show.
    pub app_icon: AppIcon,
    /// The accent colour of selected tabs, primary buttons, checkboxes,
    /// badges and focus rings. Missing in older data: orange.
    pub accent: Accent,
    /// How wide the Today tab's column is. Missing in older data: standard.
    pub today_width: TodayWidth,
    /// New tasks get a due date by default: the day they are added to
    /// (today, from the Today tab). Missing in older data: off.
    pub due_on_added_day: bool,
    /// Overdue tasks can be recommended (and then show in Recommended
    /// rather than Overdue). Missing in older data: off.
    pub recommend_overdue: bool,
    /// Last window size `(width, height)` in logical pixels. Missing in older
    /// data: the app's default window size.
    pub window_size: Option<(u32, u32)>,
}

/// The width of the Today tab's column, picked in Settings. The app maps
/// each to pixels (`theme::today_width`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodayWidth {
    #[default]
    Standard,
    Wide,
    ExtraWide,
}

impl TodayWidth {
    /// Every choice, in the order Settings shows them.
    pub const ALL: [TodayWidth; 3] = [
        TodayWidth::Standard,
        TodayWidth::Wide,
        TodayWidth::ExtraWide,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TodayWidth::Standard => "Standard",
            TodayWidth::Wide => "Wide",
            TodayWidth::ExtraWide => "Extra wide",
        }
    }
}

/// The app's accent colour, picked in Settings. The app maps each to its
/// RGB (`theme::accent_color`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    #[default]
    Orange,
    /// Olive, Sage and Green were offered briefly; saved ones load as
    /// Emerald, the one green kept.
    #[serde(alias = "olive", alias = "sage", alias = "green")]
    Emerald,
    Teal,
    Blue,
    Purple,
    Magenta,
    Red,
}

impl Accent {
    /// Every choice, in the order Settings shows them.
    pub const ALL: [Accent; 7] = [
        Accent::Orange,
        Accent::Emerald,
        Accent::Teal,
        Accent::Blue,
        Accent::Purple,
        Accent::Magenta,
        Accent::Red,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Accent::Orange => "Orange",
            Accent::Emerald => "Emerald",
            Accent::Teal => "Teal",
            Accent::Blue => "Blue",
            Accent::Purple => "Purple",
            Accent::Magenta => "Magenta",
            Accent::Red => "Red",
        }
    }
}

/// The app's colours: light or dark, or following the system appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

/// The app icon: the Dock icon on macOS, the window and taskbar icon
/// elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppIcon {
    /// Light or dark, following the system appearance.
    #[default]
    System,
    Light,
    Dark,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            llm: LlmSettings::default(),
            decision_llm: LlmSettings::default(),
            schedule: Schedule::default(),
            summary_instructions: String::new(),
            focus_minutes_per_day: DEFAULT_FOCUS_MINUTES,
            summary_prompt: String::new(),
            plan_prompt: String::new(),
            appearance: Appearance::System,
            app_icon: AppIcon::System,
            accent: Accent::Orange,
            today_width: TodayWidth::Standard,
            due_on_added_day: false,
            recommend_overdue: false,
            window_size: None,
        }
    }
}

/// Eight focus hours per working day.
pub const DEFAULT_FOCUS_MINUTES: u32 = 8 * 60;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_today_list_is_open_and_planned_by_today() {
        let today = Date::constant(2026, 11, 18);
        let task = |status, planned: Option<Date>, due: Option<Date>| Task {
            status,
            planned_for: planned,
            due_on: due,
            ..Task::default()
        };
        let mon = Some(Date::constant(2026, 11, 16));
        let wed = Some(today);
        let fri = Some(Date::constant(2026, 11, 20));
        use TaskStatus::*;
        // Planned for today, carried over from an earlier day, or undated.
        assert!(task(Todo, wed, None).on_today_list(today));
        assert!(task(Doing, mon, None).on_today_list(today));
        assert!(task(Todo, mon, fri).on_today_list(today));
        assert!(task(Todo, None, None).on_today_list(today));
        // Later or not open: not on it.
        assert!(!task(Todo, fri, None).on_today_list(today));
        assert!(!task(Done, wed, None).on_today_list(today));
        assert!(!task(Done, None, None).on_today_list(today));
        assert!(!task(Dropped, mon, None).on_today_list(today));
        // A passed deadline alone does not put a later-planned task there; that is overdue.
        let undated_overdue = task(Todo, None, mon);
        assert!(undated_overdue.on_today_list(today));
        assert!(undated_overdue.is_overdue(today));
        assert!(!task(Todo, fri, mon).on_today_list(today));
    }

    #[test]
    fn app_icon_defaults_to_system_and_round_trips() {
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.app_icon, AppIcon::System);
        let dark = Settings {
            app_icon: AppIcon::Dark,
            ..Settings::default()
        };
        let json = serde_json::to_string(&dark).unwrap();
        assert!(json.contains(r#""app_icon":"dark""#));
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.app_icon, AppIcon::Dark);
    }

    #[test]
    fn today_width_is_standard_in_older_data_and_round_trips() {
        let old: Settings = serde_json::from_str(r#"{"accent":"teal"}"#).unwrap();
        assert_eq!(old.today_width, TodayWidth::Standard);
        assert!(!old.due_on_added_day);
        assert!(!old.recommend_overdue);
        let wide = Settings {
            today_width: TodayWidth::ExtraWide,
            ..Settings::default()
        };
        let json = serde_json::to_string(&wide).unwrap();
        assert!(json.contains(r#""today_width":"extra_wide""#));
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.today_width, TodayWidth::ExtraWide);
    }

    #[test]
    fn weeks_start_on_sunday_unless_saved_otherwise() {
        assert_eq!(Settings::default().schedule.week_start, Weekday::Sunday);
        let old: Settings = serde_json::from_str(r#"{"accent":"teal"}"#).unwrap();
        assert_eq!(old.schedule.week_start, Weekday::Sunday);
    }

    #[test]
    fn appearance_defaults_to_system_and_round_trips() {
        let old: Settings = serde_json::from_str(r#"{"accent":"teal"}"#).unwrap();
        assert_eq!(old.appearance, Appearance::System);
        let s = Settings {
            appearance: Appearance::Dark,
            ..Settings::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""appearance":"dark""#));
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.appearance, Appearance::Dark);
    }

    #[test]
    fn accent_defaults_to_orange_and_round_trips() {
        let old: Settings = serde_json::from_str(r#"{"app_icon":"dark"}"#).unwrap();
        assert_eq!(old.accent, Accent::Orange);
        let teal = Settings {
            accent: Accent::Teal,
            ..Settings::default()
        };
        let json = serde_json::to_string(&teal).unwrap();
        assert!(json.contains(r#""accent":"teal""#));
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), teal);
        // Greens no longer offered load as Emerald.
        for old in ["olive", "sage", "green"] {
            let s: Settings = serde_json::from_str(&format!(r#"{{"accent":"{old}"}}"#)).unwrap();
            assert_eq!(s.accent, Accent::Emerald);
        }
    }

    #[test]
    fn settings_round_trip_through_json() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), s);
        assert_eq!(serde_json::from_str::<Settings>("{}").unwrap(), s);
    }

    #[test]
    fn round1_settings_json_loads_with_new_defaults() {
        let json = r#"{"llm":{"base_url":"http://x/v1","model":"m"},"schedule":{"week_start":1,"summary_day":5,"summary_time":"16:00:00","plan_day":1,"plan_time":"08:00:00","working_days":[1,2,3,4]},"summary_instructions":"short"}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.llm.protocol, Protocol::OpenAi);
        assert_eq!(s.llm.base_url, "http://x/v1");
        assert_eq!(s.schedule.working_days.len(), 4);
        // The old day and time fields are ignored; a saved week start stays.
        assert_eq!(s.schedule.week_start, Weekday::Monday);
        assert_eq!(s.summary_instructions, "short");
        assert_eq!(s.focus_minutes_per_day, DEFAULT_FOCUS_MINUTES);
        assert!(!s.decision_llm.is_configured());
        assert!(!s.llm.keyless);
    }

    #[test]
    fn settings_json_with_the_removed_notifications_field_still_loads() {
        let json = r#"{"llm":{"base_url":"http://x/v1","model":"m"},"notifications":true,"summary_instructions":"short","accent":"teal"}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.llm.base_url, "http://x/v1");
        assert_eq!(s.summary_instructions, "short");
        assert_eq!(s.accent, Accent::Teal);
    }

    #[test]
    fn event_kind_json_is_tagged() {
        let k = EventKind::StatusChanged {
            from: TaskStatus::Todo,
            to: TaskStatus::Done,
        };
        let json = serde_json::to_string(&k).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"status_changed","from":"todo","to":"done"}"#
        );
        assert_eq!(serde_json::from_str::<EventKind>(&json).unwrap(), k);
        assert_eq!(k.as_str(), "status_changed");
    }

    #[test]
    fn task_day_is_the_planned_day_only() {
        let mut t = Task {
            due_on: Some(jiff::civil::date(2026, 11, 20)),
            ..Task::default()
        };
        assert_eq!(t.day(), None);
        t.planned_for = Some(jiff::civil::date(2026, 11, 18));
        assert_eq!(t.day(), t.planned_for);
        assert!(t.is_overdue(jiff::civil::date(2026, 11, 21)));
        t.status = TaskStatus::Done;
        assert!(!t.is_overdue(jiff::civil::date(2026, 11, 21)));
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

    #[test]
    fn recurrence_next_date_and_occurrence() {
        use jiff::civil::date;
        let fri = date(2026, 11, 20);
        assert_eq!(Recurrence::Daily.next_date(fri), date(2026, 11, 21));
        assert_eq!(Recurrence::Weekdays.next_date(fri), date(2026, 11, 23));
        assert_eq!(Recurrence::Weekly.next_date(fri), date(2026, 11, 27));
        assert_eq!(Recurrence::Monthly.next_date(fri), date(2026, 12, 20));
        assert_eq!(
            Recurrence::Monthly.next_date(date(2026, 1, 31)),
            date(2026, 2, 28)
        );

        let task = Task {
            title: "Weekly review".into(),
            notes: "Prep deck\n\n- [x] pull metrics\n- [x] send email".into(),
            project: Some("work".into()),
            planned_for: Some(date(2026, 11, 16)),
            due_on: Some(date(2026, 11, 18)),
            estimate_minutes: Some(60),
            priority: 2,
            private: true,
            recurrence: Some(Recurrence::Weekly),
            ..Task::default()
        };
        let next = next_occurrence(&task, date(2026, 11, 18)).unwrap();
        assert_eq!(next.planned_for, Some(date(2026, 11, 25)));
        assert_eq!(next.due_on, Some(date(2026, 11, 27)));
        assert_eq!(
            next.notes,
            "Prep deck\n\n- [ ] pull metrics\n- [ ] send email"
        );
        assert_eq!(next.recurrence, Some(Recurrence::Weekly));
        assert!(next.private);
    }
}
