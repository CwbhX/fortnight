//! Fortnight: the application shell. Owns all state, carries out the actions
//! the views return, talks to the store, the key vault and the LLM, and runs
//! the minute tick that generates due reports.
//!
//! Layout (PLAN.md 4.2): the top bar (tabs, period navigation over one
//! shared selected date, lock, gear, quick-add), then the active tab. Settings
//! opens over the current view.
//!
//! Also here: undo (Ctrl/Cmd+Z: the last add, status change, edit or
//! delete), the keyboard shortcuts of PLAN.md 4.4, OS notifications for
//! scheduled reports, Lock, the data-folder lock file, the export dialog and
//! the model list.

use fortnight_app::theme;
use fortnight_app::top_bar::{self, LockIcon, Tab};
use fortnight_app::views::NavKey;
use fortnight_app::views::day::{self, DayData, DayView, LogLine};
use fortnight_app::views::month::{self, MonthData, MonthView};
use fortnight_app::views::report_panel::{self, ReportPanelData, ReportSlot};
use fortnight_app::views::settings::{
    self, KeyState, SettingsData, SettingsView, UnlockAction, UnlockMessage, UnlockView,
};
use fortnight_app::views::week::{self, WeekData, WeekView};
use fortnight_core::datadir::{self, DataLock};
use fortnight_core::domain::{
    DeletedTask, Event, LlmSettings, ReportId, ReportKey, ReportKind, Settings, Task as CoreTask,
    TaskId, TaskStatus, WeekRef,
};
use fortnight_core::export::{self, ExportFormat};
use fortnight_core::llm::{
    self, ChatEvent, ChatRequest, ChatResponse, LlmClient, LlmConfig, LlmError,
};
use fortnight_core::report::{self, Prepared};
use fortnight_core::sqlite::SqliteStore;
use fortnight_core::store::Store;
use fortnight_core::vault::{self, VaultError};
use fortnight_core::week::{due_reports, week_containing, week_label};
use iced::keyboard::{self, Key, key::Named};
use iced::widget::{
    button, center, column, container, mouse_area, opaque, operation, row, stack, text,
};
use iced::{Element, Fill, Subscription, Task, Theme};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ZonedDateTime};
use secrecy::SecretString;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// A failed automatic generation is not retried for this long.
const RETRY_AFTER_MINUTES: i64 = 10;

pub fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("Fortnight")
        // One bundled typeface (Inter, OFL), so bold and italic Markdown
        // render the same on every machine.
        .font(include_bytes!("../../../assets/fonts/Inter-Regular.ttf").as_slice())
        .font(include_bytes!("../../../assets/fonts/Inter-Bold.ttf").as_slice())
        .font(include_bytes!("../../../assets/fonts/Inter-Italic.ttf").as_slice())
        .font(include_bytes!("../../../assets/fonts/Inter-BoldItalic.ttf").as_slice())
        .default_font(theme::FONT)
        .theme(App::theme)
        .subscription(App::subscription)
        .window_size((1100.0, 760.0))
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    Top(top_bar::Message),
    Day(day::Message),
    Week(week::Message),
    Month(month::Message),
    Settings(settings::Message),
    Unlock(UnlockMessage),
    CloseSettings,
    Tick,
    ThemeMode(iced::theme::Mode),
    /// A piece of a streamed report.
    Stream(ReportKey, Result<ChatEvent, LlmError>),
    /// The stream for this report ended (after `Done` or an error, or early).
    StreamEnded(ReportKey),
    Tested(Result<String, String>),
    Models(Result<Vec<String>, String>),
    /// A keyboard shortcut (applied only where it makes sense).
    Key(Shortcut),
    Undo,
    /// The export dialog finished: the path written, `None` if cancelled.
    Exported(Result<Option<PathBuf>, String>),
    DismissNotice,
    Unlocked(Result<SecretString, VaultError>),
    KeySaved(Result<SecretString, VaultError>),
    PasswordChanged(Result<(), VaultError>),
    DismissError,
    Shoot,
    Shot(iced::window::Screenshot),
}

/// A key pressed while no widget captured it (PLAN.md 4.4). Single keys
/// arrive only when no text field has focus, because a focused text input
/// captures them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shortcut {
    Tab(Tab),
    Prev,
    Next,
    Today,
    FocusQuickAdd,
    Settings,
    Undo,
    Escape,
    Nav(NavKey),
}

/// A report being generated: what was asked, and the text so far.
struct Generation {
    prepared: Prepared,
    text: String,
    requested_model: String,
    host: Option<String>,
    /// Started by the schedule (not a button): notify when it is ready.
    scheduled: bool,
    /// Aborts the request (Lock stops requests that hold the key).
    abort: iced::task::Handle,
}

/// One change that Ctrl/Cmd+Z can take back.
#[derive(Debug, Clone)]
enum Undo {
    /// A task was added: delete it.
    Added(TaskId),
    /// A task's status changed from this one.
    Status(TaskId, TaskStatus),
    /// A task was edited; this is how it was before.
    Edited(Box<CoreTask>),
    /// A task was deleted; put it back with its history.
    Deleted(Box<DeletedTask>),
}

/// How many changes undo remembers.
const UNDO_DEPTH: usize = 30;

/// Everything that needs the open database.
struct Ready {
    store: SqliteStore,
    data_dir: PathBuf,
    settings: Settings,
    tasks: Vec<CoreTask>,
    /// Every event, oldest first. Reloaded with the tasks.
    events: Vec<Event>,
    tz: TimeZone,

    tab: Tab,
    /// The one selected date all three tabs zoom around.
    selected: Date,
    quick_add: String,
    settings_open: bool,

    day_view: DayView,
    week_view: WeekView,
    month_view: MonthView,
    settings_view: SettingsView,
    unlock_view: UnlockView,
    /// The unlock prompt is showing (launch with a key file, until unlocked
    /// or skipped; or the lock icon while locked).
    unlocking: bool,

    key_exists: bool,
    api_key: Option<SecretString>,

    generating: HashMap<ReportKey, Generation>,
    last_attempt: HashMap<ReportKey, Timestamp>,
    testing: bool,
    test_result: Option<Result<String, String>>,
    key_message: Option<Result<String, String>>,
    models: Option<Result<Vec<String>, String>>,
    listing_models: bool,
    error: Option<String>,
    /// A short, dismissable message (undo hint, export result, lock warning).
    notice: Option<String>,
    undo: Vec<Undo>,
    /// A report became ready while the Week tab wasn't showing.
    week_badge: bool,
    /// Test connection / Load models requests in flight, aborted on Lock.
    key_requests: Vec<iced::task::Handle>,
    /// Held for the app's lifetime; removes the lock file when dropped.
    _lock: Option<DataLock>,
}

enum App {
    /// The data folder or database could not be opened.
    Fatal(String),
    Ready(Box<Ready>, Option<bool>),
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        match Ready::open() {
            Ok(ready) => {
                let mut ready = Box::new(ready);
                let start = ready.run_due();
                ready.refresh();
                let tasks = Task::batch([
                    iced::system::theme().map(Message::ThemeMode),
                    if ready.unlocking {
                        UnlockView::focus().map(Message::Unlock)
                    } else {
                        operation::focus(top_bar::QUICK_ADD_ID)
                    },
                    start,
                    shoot_after_delay(),
                ]);
                (App::Ready(ready, forced_theme()), tasks)
            }
            Err(e) => (App::Fatal(e), shoot_after_delay()),
        }
    }

    fn theme(&self) -> Option<Theme> {
        match self {
            App::Ready(_, Some(dark)) => Some(theme::theme_for(*dark)),
            _ => None,
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            iced::time::every(Duration::from_secs(60)).map(|_| Message::Tick),
            iced::system::theme_changes().map(Message::ThemeMode),
            keyboard::listen().filter_map(|event| {
                let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
                    return None;
                };
                shortcut_for(key.as_ref(), modifiers).map(Message::Key)
            }),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match (&mut *self, message) {
            (_, Message::Shoot) => iced::window::latest()
                .and_then(iced::window::screenshot)
                .map(Message::Shot),
            (_, Message::Shot(shot)) => save_screenshot(shot),
            (App::Ready(_, _), Message::ThemeMode(_)) if forced_theme().is_some() => Task::none(),
            (App::Ready(_, mode), Message::ThemeMode(m)) => {
                *mode = match m {
                    iced::theme::Mode::Dark => Some(true),
                    iced::theme::Mode::Light => Some(false),
                    iced::theme::Mode::None => None,
                };
                Task::none()
            }
            (App::Ready(ready, _), message) => ready.update(message),
            (App::Fatal(_), _) => Task::none(),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        match self {
            App::Fatal(e) => center(
                column![
                    text("Fortnight cannot open its data folder").size(22),
                    text(e.as_str()),
                    text("Start it with --data-dir <folder> or set FORTNIGHT_DATA_DIR."),
                ]
                .spacing(theme::SPACING)
                .max_width(560),
            )
            .into(),
            App::Ready(ready, _) => ready.view(),
        }
    }
}

impl Ready {
    fn open() -> Result<Ready, String> {
        let dir = datadir::resolve(data_dir_flag()).map_err(|e| e.to_string())?;
        datadir::ensure(&dir).map_err(|e| e.to_string())?;
        let store = SqliteStore::open(&datadir::db_path(&dir)).map_err(|e| e.to_string())?;
        let settings = store.load_settings().map_err(|e| e.to_string())?;
        let tasks = store.list_tasks().map_err(|e| e.to_string())?;
        let events = store
            .events_between(Timestamp::MIN, Timestamp::MAX)
            .map_err(|e| e.to_string())?;
        let tz = TimeZone::system();
        let today = Timestamp::now().to_zoned_datetime(tz.clone()).date();
        let key_exists = vault::key_path(&dir).exists();
        let (tab, settings_open) = match std::env::var("FORTNIGHT_PAGE").as_deref() {
            Ok("week") => (Tab::Week, false),
            Ok("month") => (Tab::Month, false),
            Ok("settings") => (Tab::Today, true),
            _ => (Tab::Today, false),
        };
        let unlocking = key_exists && std::env::var_os("FORTNIGHT_SKIP_UNLOCK").is_none();
        // One writer at a time (PLAN.md 3.5): warn, never refuse.
        let (lock, notice) = match datadir::acquire_lock(&dir) {
            Ok((lock, None)) => (Some(lock), None),
            Ok((lock, Some(previous))) => {
                let since = previous
                    .since
                    .to_zoned_datetime(tz.clone())
                    .strftime("%a %-d %b %H:%M")
                    .to_string();
                (
                    Some(lock),
                    Some(format!(
                        "This data folder was already open on {} (since {since}), or Fortnight \
                         didn't close cleanly. Don't use one folder on two machines at once.",
                        previous.host
                    )),
                )
            }
            Err(e) => (
                None,
                Some(format!("Could not take the data folder lock: {e}")),
            ),
        };
        Ok(Ready {
            store,
            data_dir: dir,
            settings,
            tasks,
            events,
            tz,
            tab,
            selected: today,
            quick_add: String::new(),
            settings_open,
            day_view: DayView::new(),
            week_view: WeekView::new(),
            month_view: MonthView::new(),
            settings_view: SettingsView::new(),
            unlock_view: UnlockView::new(),
            unlocking,
            key_exists,
            api_key: None,
            generating: HashMap::new(),
            last_attempt: HashMap::new(),
            testing: false,
            test_result: None,
            key_message: None,
            models: None,
            listing_models: false,
            error: None,
            notice,
            undo: Vec::new(),
            week_badge: false,
            key_requests: Vec::new(),
            _lock: lock,
        })
    }

    fn now(&self) -> ZonedDateTime {
        Timestamp::now().to_zoned_datetime(self.tz.clone())
    }

    fn today(&self) -> Date {
        self.now().date()
    }

    fn week_of(&self, date: Date) -> WeekRef {
        week_containing(date, self.settings.schedule.week_start)
    }

    fn current_week(&self) -> WeekRef {
        self.week_of(self.today())
    }

    /// Why the LLM can't be used right now, if it can't.
    fn blocked(&self) -> Option<String> {
        if !self.settings.llm.is_configured() {
            Some("Set up an endpoint in Settings to generate.".into())
        } else if self.key_exists && self.api_key.is_none() {
            Some("Unlock the API key to generate.".into())
        } else {
            None
        }
    }

    fn client_for(
        &self,
        llm: &LlmSettings,
    ) -> Result<(Arc<dyn LlmClient>, Option<String>), String> {
        if self.key_exists && self.api_key.is_none() {
            return Err("The API key is locked. Unlock it first.".into());
        }
        let config = LlmConfig {
            base_url: llm.base_url.trim().to_string(),
            model: llm.model.trim().to_string(),
            api_key: self.api_key.clone(),
        };
        let host = config.endpoint_host();
        let client = llm::connect(llm.protocol, config).map_err(|e| e.to_string())?;
        Ok((client, host))
    }

    // ---- data flow ----

    fn reload_tasks(&mut self) {
        match self.store.list_tasks() {
            Ok(tasks) => self.tasks = tasks,
            Err(e) => self.error = Some(e.to_string()),
        }
        match self.store.events_between(Timestamp::MIN, Timestamp::MAX) {
            Ok(events) => self.events = events,
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Pushes fresh data into the visible tab (and Settings when open).
    fn refresh(&mut self) {
        let today = self.today();
        match self.tab {
            Tab::Today => {
                let logs = self
                    .events
                    .iter()
                    .filter_map(|e| {
                        Some(LogLine {
                            id: e.id,
                            task_id: e.task_id,
                            at: e.at,
                            date: self.tz.to_datetime(e.at).date(),
                            text: e.log_text()?.to_string(),
                        })
                    })
                    .collect();
                self.day_view.load(DayData {
                    day: self.selected,
                    today,
                    tasks: self.tasks.clone(),
                    logs,
                });
            }
            Tab::Week => {
                let week = self.week_of(self.selected);
                let panel = self.panel_data(week);
                self.week_view.load(WeekData {
                    week,
                    today,
                    tasks: self.tasks.clone(),
                    working_days: self.settings.schedule.working_days.clone(),
                    loads: report::day_loads(&self.tasks, week, &self.settings),
                    panel,
                });
            }
            Tab::Month => {
                let summary_weeks = match self.store.ready_report_keys() {
                    Ok(keys) => keys
                        .into_iter()
                        .filter(|k| k.kind == ReportKind::Summary)
                        .map(|k| k.week_start)
                        .collect(),
                    Err(e) => {
                        self.error = Some(e.to_string());
                        Vec::new()
                    }
                };
                self.month_view.load(MonthData {
                    month: self.selected.first_of_month(),
                    today,
                    week_start: self.settings.schedule.week_start,
                    working_days: self.settings.schedule.working_days.clone(),
                    tasks: self.tasks.clone(),
                    summary_weeks,
                });
            }
        }

        if self.settings_open {
            self.settings_view.load(SettingsData {
                settings: self.settings.clone(),
                data_dir: self.data_dir.clone(),
                key_state: self.key_state(),
                test_result: self.test_result.clone(),
                testing: self.testing,
                key_message: self.key_message.clone(),
                models: self.models.clone(),
                listing_models: self.listing_models,
            });
        }
    }

    /// Which reports the panel offers for `week` (PLAN.md 4.2): a past week
    /// its Summary; the current week Advice, plus the Summary once due; a
    /// future week Advice.
    fn panel_kinds(&self, week: WeekRef) -> Vec<ReportKind> {
        let current = self.current_week();
        if week < current {
            vec![ReportKind::Summary]
        } else if week > current {
            vec![ReportKind::Plan]
        } else if self.summary_due(current) {
            vec![ReportKind::Summary, ReportKind::Plan]
        } else {
            vec![ReportKind::Plan]
        }
    }

    /// The summary for `week` is due (its summary day and time have passed).
    fn summary_due(&self, week: WeekRef) -> bool {
        due_reports(&self.now(), &self.settings.schedule, &[])
            .iter()
            .any(|job| job.kind == ReportKind::Summary && job.week == week)
    }

    fn prepare(&self, kind: ReportKind, week: WeekRef) -> Prepared {
        let today = self.today();
        match kind {
            ReportKind::Summary => report::prepare_summary(
                &self.tasks,
                &self.events,
                week,
                today,
                &self.tz,
                &self.settings,
            ),
            ReportKind::Plan => {
                report::prepare_plan(&self.tasks, &self.events, week, today, &self.settings)
            }
        }
    }

    fn panel_data(&mut self, week: WeekRef) -> ReportPanelData {
        let today = self.today();
        let blocked = self.blocked();
        let mut slots = Vec::new();
        for kind in self.panel_kinds(week) {
            let facts_md = match kind {
                ReportKind::Summary => report::summary_facts_markdown(&report::summary_facts(
                    &self.tasks,
                    &self.events,
                    week,
                    today,
                    &self.tz,
                )),
                ReportKind::Plan => report::plan_facts_markdown(&report::plan_facts(
                    &self.tasks,
                    &self.events,
                    week,
                    today,
                    &self.settings,
                )),
            };
            let preview = report::request_preview(&self.prepare(kind, week).request);
            let versions = match self.store.list_reports(kind, week.start) {
                Ok(v) => v,
                Err(e) => {
                    self.error = Some(e.to_string());
                    Vec::new()
                }
            };
            let key = ReportKey {
                kind,
                week_start: week.start,
            };
            slots.push(ReportSlot {
                kind,
                facts_md,
                preview,
                versions,
                streaming: self.generating.get(&key).map(|g| g.text.clone()),
                blocked: blocked.clone(),
            });
        }
        ReportPanelData {
            week,
            week_label: week_label(week),
            slots,
        }
    }

    fn key_state(&self) -> KeyState {
        match (self.key_exists, self.api_key.is_some()) {
            (false, _) => KeyState::NoKey,
            (true, false) => KeyState::Locked,
            (true, true) => KeyState::Unlocked,
        }
    }

    /// Starts streaming a report unless one is already in flight.
    fn generate(&mut self, kind: ReportKind, week: WeekRef, scheduled: bool) -> Task<Message> {
        let key = ReportKey {
            kind,
            week_start: week.start,
        };
        if self.generating.contains_key(&key) {
            return Task::none();
        }
        let (client, host) = match self.client_for(&self.settings.llm.clone()) {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(e);
                return Task::none();
            }
        };
        let prepared = self.prepare(kind, week);
        let stream = client.chat_stream(prepared.request.clone());
        let (task, abort) = Task::run(stream, move |item| Message::Stream(key, item))
            .chain(Task::done(Message::StreamEnded(key)))
            .abortable();
        self.generating.insert(
            key,
            Generation {
                prepared,
                text: String::new(),
                requested_model: client.model().to_string(),
                host,
                scheduled,
                abort,
            },
        );
        self.last_attempt.insert(key, Timestamp::now());
        task
    }

    /// Stores the finished (or failed) report and forgets the generation.
    /// A scheduled report that became ready sets the Week badge and, if
    /// enabled, shows an OS notification.
    fn finish(&mut self, key: ReportKey, result: Result<ChatResponse, LlmError>) -> Task<Message> {
        let Some(g) = self.generating.remove(&key) else {
            return Task::none();
        };
        let ok = result.is_ok();
        let new = report::finish(g.prepared, result, &g.requested_model, g.host);
        if let Err(e) = self.store.insert_report(new) {
            self.error = Some(e.to_string());
            return Task::none();
        }
        if !(ok && g.scheduled) {
            return Task::none();
        }
        if self.tab != Tab::Week {
            self.week_badge = true;
        }
        if !self.settings.notifications {
            return Task::none();
        }
        let body = match key.kind {
            ReportKind::Summary => "Your weekly summary is ready.",
            ReportKind::Plan => "Advice for your week is ready.",
        };
        // Best-effort (PLAN.md 3.5): failures are ignored.
        Task::perform(
            async move {
                let _ = tokio::task::spawn_blocking(move || {
                    notify_rust::Notification::new()
                        .summary("Fortnight")
                        .body(body)
                        .show()
                        .map(|_| ())
                })
                .await;
            },
            |()| Message::DismissNotice,
        )
        .discard()
    }

    /// Remembers a change for undo.
    fn remember(&mut self, undo: Undo) {
        self.undo.push(undo);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
    }

    /// Takes back the last change (Ctrl/Cmd+Z).
    fn undo(&mut self) {
        let Some(last) = self.undo.pop() else {
            self.notice = Some("Nothing to undo.".into());
            return;
        };
        let (result, what) = match last {
            Undo::Added(id) => (
                self.store.delete_task(id).map(|_| ()),
                "Removed the new task.",
            ),
            Undo::Status(id, before) => (self.store.set_status(id, before), "Status restored."),
            Undo::Edited(before) => (self.store.update_task(&before), "Edit undone."),
            Undo::Deleted(deleted) => (self.store.restore_task(&deleted), "Task restored."),
        };
        match result {
            Ok(()) => self.notice = Some(what.into()),
            Err(e) => self.error = Some(format!("Could not undo: {e}")),
        }
        self.reload_tasks();
    }

    fn task(&self, id: TaskId) -> Option<&CoreTask> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// The schedule: generates whatever is due (PLAN.md section 8). Runs at
    /// launch and every minute. Waits quietly while locked or unconfigured;
    /// a failed attempt is retried after `RETRY_AFTER_MINUTES`.
    fn run_due(&mut self) -> Task<Message> {
        if self.blocked().is_some() || self.tasks.is_empty() {
            return Task::none();
        }
        let existing = match self.store.ready_report_keys() {
            Ok(k) => k,
            Err(e) => {
                self.error = Some(e.to_string());
                return Task::none();
            }
        };
        let now = Timestamp::now();
        let jobs = due_reports(&self.now(), &self.settings.schedule, &existing);
        let mut tasks = Vec::new();
        for job in jobs {
            let key = ReportKey {
                kind: job.kind,
                week_start: job.week.start,
            };
            let recently_tried = self.last_attempt.get(&key).is_some_and(|t| {
                now.duration_since(*t) < SignedDuration::from_mins(RETRY_AFTER_MINUTES)
            });
            if !recently_tried {
                tasks.push(self.generate(job.kind, job.week, true));
            }
        }
        Task::batch(tasks)
    }

    // ---- update ----

    fn update(&mut self, message: Message) -> Task<Message> {
        let task = match message {
            Message::Top(m) => self.top_bar(m),
            Message::Day(m) => {
                let action = self.day_view.update(m);
                self.day_action(action)
            }
            Message::Week(m) => match self.week_view.update(m) {
                week::Action::None => Task::none(),
                week::Action::SetStatus(id, status) => {
                    self.set_status(id, status);
                    Task::none()
                }
                week::Action::OpenDay(day) => {
                    self.selected = day;
                    self.tab = Tab::Today;
                    Task::none()
                }
                week::Action::Panel(action) => self.panel_action(action),
            },
            Message::Month(m) => match self.month_view.update(m) {
                month::Action::None => Task::none(),
                month::Action::OpenDay(day) => {
                    self.selected = day;
                    self.tab = Tab::Today;
                    Task::none()
                }
                month::Action::OpenWeek(week) => {
                    self.selected = week.start;
                    self.tab = Tab::Week;
                    Task::none()
                }
            },
            Message::Settings(m) => {
                let action = self.settings_view.update(m);
                self.settings_action(action)
            }
            Message::CloseSettings => {
                self.settings_open = false;
                Task::none()
            }
            Message::Unlock(m) => match self.unlock_view.update(m) {
                UnlockAction::None => Task::none(),
                UnlockAction::Skip => {
                    self.unlocking = false;
                    Task::none()
                }
                UnlockAction::Unlock(password) => self.unlock(password),
            },
            Message::Tick => self.run_due(),
            Message::Stream(key, item) => match item {
                Ok(ChatEvent::Delta(d)) if self.generating.contains_key(&key) => {
                    if let Some(g) = self.generating.get_mut(&key) {
                        g.text.push_str(&d);
                    }
                    Task::none()
                }
                Ok(ChatEvent::Done { model }) => {
                    let text = self
                        .generating
                        .get(&key)
                        .map(|g| g.text.clone())
                        .unwrap_or_default();
                    self.finish(key, Ok(ChatResponse { text, model }))
                }
                Err(e) => self.finish(key, Err(e)),
                Ok(ChatEvent::Delta(_)) => Task::none(),
            },
            Message::StreamEnded(key) => {
                // Only still generating if the stream ended without `Done`.
                self.finish(key, Err(LlmError::Truncated))
            }
            Message::Tested(result) => {
                self.testing = false;
                if !self.listing_models {
                    self.key_requests.clear();
                }
                self.test_result = Some(result);
                Task::none()
            }
            Message::Models(result) => {
                self.listing_models = false;
                if !self.testing {
                    self.key_requests.clear();
                }
                self.models = Some(result);
                Task::none()
            }
            Message::Key(shortcut) => self.shortcut(shortcut),
            Message::Undo => {
                self.undo();
                Task::none()
            }
            Message::Exported(result) => {
                match result {
                    Ok(Some(path)) => {
                        self.notice = Some(format!("Exported to {}", path.display()));
                    }
                    Ok(None) => {}
                    Err(e) => self.error = Some(format!("Export failed: {e}")),
                }
                Task::none()
            }
            Message::DismissNotice => {
                self.notice = None;
                Task::none()
            }
            Message::Unlocked(result) => {
                match result {
                    Ok(key) => {
                        self.api_key = Some(key);
                        self.unlocking = false;
                        self.key_message = Some(Ok("Unlocked.".into()));
                    }
                    Err(e) => {
                        self.unlock_view.set_error(e.to_string());
                        self.key_message = Some(Err(e.to_string()));
                    }
                }
                self.run_due()
            }
            Message::KeySaved(result) => {
                match result {
                    Ok(key) => {
                        self.api_key = Some(key);
                        self.key_exists = true;
                        self.key_message = Some(Ok("API key saved and encrypted.".into()));
                    }
                    Err(e) => self.key_message = Some(Err(e.to_string())),
                }
                self.run_due()
            }
            Message::PasswordChanged(result) => {
                self.key_message = Some(match result {
                    Ok(()) => Ok("Password changed.".into()),
                    Err(e) => Err(e.to_string()),
                });
                Task::none()
            }
            Message::DismissError => {
                self.error = None;
                Task::none()
            }
            Message::ThemeMode(_) | Message::Shoot | Message::Shot(_) => Task::none(),
        };
        self.refresh();
        task
    }

    fn top_bar(&mut self, message: top_bar::Message) -> Task<Message> {
        match message {
            top_bar::Message::Tab(tab) => {
                self.tab = tab;
                if tab == Tab::Week {
                    self.week_badge = false;
                }
                Task::none()
            }
            top_bar::Message::Prev => {
                self.selected = top_bar::step(self.tab, self.selected, -1);
                Task::none()
            }
            top_bar::Message::Next => {
                self.selected = top_bar::step(self.tab, self.selected, 1);
                Task::none()
            }
            top_bar::Message::Today => {
                self.selected = self.today();
                Task::none()
            }
            top_bar::Message::Lock => {
                if self.api_key.is_some() {
                    self.lock();
                    Task::none()
                } else {
                    self.unlocking = true;
                    UnlockView::focus().map(Message::Unlock)
                }
            }
            top_bar::Message::Settings => {
                self.settings_open = !self.settings_open;
                Task::none()
            }
            top_bar::Message::QuickAdd(s) => {
                self.quick_add = s;
                Task::none()
            }
            top_bar::Message::QuickAddSubmit => {
                let today = self.today();
                if let Some(new) =
                    top_bar::quick_add_task(&self.quick_add, self.tab, self.selected, today)
                {
                    match self.store.add_task(new) {
                        Ok(task) => {
                            self.quick_add.clear();
                            self.remember(Undo::Added(task.id));
                        }
                        Err(e) => self.error = Some(e.to_string()),
                    }
                    self.reload_tasks();
                }
                Task::none()
            }
        }
    }

    fn set_status(&mut self, id: TaskId, status: TaskStatus) {
        let before = self.task(id).map(|t| t.status);
        match self.store.set_status(id, status) {
            Ok(()) => {
                if let Some(before) = before.filter(|b| *b != status) {
                    self.remember(Undo::Status(id, before));
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.reload_tasks();
    }

    fn day_action(&mut self, action: day::Action) -> Task<Message> {
        let result = match action {
            day::Action::None => return Task::none(),
            day::Action::SetStatus(id, status) => {
                self.set_status(id, status);
                return Task::none();
            }
            day::Action::Update(task) => {
                let before = self.task(task.id).cloned();
                let r = self.store.update_task(&task);
                if let (Ok(()), Some(before)) = (&r, before) {
                    self.remember(Undo::Edited(Box::new(before)));
                }
                r
            }
            day::Action::Delete(id) => match self.store.delete_task(id) {
                Ok(deleted) => {
                    self.notice = Some(format!(
                        "Deleted “{}”. Press {}Z to undo.",
                        deleted.task.title,
                        if cfg!(target_os = "macos") {
                            "⌘"
                        } else {
                            "Ctrl+"
                        }
                    ));
                    self.remember(Undo::Deleted(Box::new(deleted)));
                    Ok(())
                }
                Err(e) => Err(e),
            },
            day::Action::AddLog { task_id, text } => self.store.add_log(task_id, &text).map(|_| ()),
            day::Action::DeleteLog(id) => self.store.delete_log(id),
        };
        if let Err(e) = result {
            self.error = Some(e.to_string());
        }
        self.reload_tasks();
        Task::none()
    }

    fn panel_action(&mut self, action: report_panel::Action) -> Task<Message> {
        match action {
            report_panel::Action::None => Task::none(),
            report_panel::Action::Generate(kind, week) => self.generate(kind, week, false),
            report_panel::Action::SaveEdit(id, text) => {
                if let Err(e) = self.store.set_report_edit(id, Some(&text)) {
                    self.error = Some(e.to_string());
                }
                Task::none()
            }
            report_panel::Action::Copy(text) => iced::clipboard::write(text),
            report_panel::Action::Export(id, format) => self.export_report(id, format),
        }
    }

    /// Asks where to save one report version, then writes it.
    fn export_report(&mut self, id: ReportId, format: ExportFormat) -> Task<Message> {
        let report = match self.store.all_reports() {
            Ok(all) => all.into_iter().find(|r| r.id == id),
            Err(e) => {
                self.error = Some(e.to_string());
                return Task::none();
            }
        };
        let Some(report) = report else {
            self.error = Some(format!("Report {id} not found."));
            return Task::none();
        };
        let name = export::report_file_name(format, &report);
        let content = export::export_report(format, &report);
        Task::perform(
            async move {
                let Some(file) = rfd::AsyncFileDialog::new()
                    .set_title("Export report")
                    .set_file_name(&name)
                    .add_filter(format.extension(), &[format.extension()])
                    .save_file()
                    .await
                else {
                    return Ok(None);
                };
                let path = file.path().to_path_buf();
                std::fs::write(&path, content)
                    .map(|()| Some(path))
                    .map_err(|e| e.to_string())
            },
            Message::Exported,
        )
    }

    /// Applies a keyboard shortcut. While the unlock prompt or Settings is
    /// showing, only the keys that close or toggle them apply.
    fn shortcut(&mut self, shortcut: Shortcut) -> Task<Message> {
        if self.unlocking {
            return Task::none();
        }
        if self.settings_open {
            return match shortcut {
                Shortcut::Escape | Shortcut::Settings => {
                    self.settings_open = false;
                    Task::none()
                }
                _ => Task::none(),
            };
        }
        match shortcut {
            Shortcut::Tab(tab) => self.top_bar(top_bar::Message::Tab(tab)),
            Shortcut::Prev => self.top_bar(top_bar::Message::Prev),
            Shortcut::Next => self.top_bar(top_bar::Message::Next),
            Shortcut::Today => self.top_bar(top_bar::Message::Today),
            Shortcut::FocusQuickAdd => operation::focus(top_bar::QUICK_ADD_ID),
            Shortcut::Settings => {
                self.settings_open = true;
                Task::none()
            }
            Shortcut::Undo => {
                self.undo();
                Task::none()
            }
            Shortcut::Escape => {
                if self.tab == Tab::Today {
                    let action = self.day_view.on_key(NavKey::Close);
                    self.day_action(action)
                } else {
                    Task::none()
                }
            }
            Shortcut::Nav(key) => {
                if self.tab == Tab::Today {
                    let action = self.day_view.on_key(key);
                    self.day_action(action)
                } else {
                    Task::none()
                }
            }
        }
    }

    /// Forgets the key and stops every request that holds a copy of it.
    /// Reports being generated are dropped (not stored); the schedule
    /// generates them again after the next unlock.
    fn lock(&mut self) {
        self.api_key = None; // dropping the SecretString zeroizes it
        for (_, g) in self.generating.drain() {
            g.abort.abort();
        }
        for handle in self.key_requests.drain(..) {
            handle.abort();
        }
        self.testing = false;
        self.listing_models = false;
        self.key_message = Some(Ok("Locked.".into()));
    }

    fn settings_action(&mut self, action: settings::Action) -> Task<Message> {
        let key_path = vault::key_path(&self.data_dir);
        match action {
            settings::Action::None => Task::none(),
            settings::Action::Close => {
                self.settings_open = false;
                Task::none()
            }
            settings::Action::Save(s) => {
                match self.store.save_settings(&s) {
                    Ok(()) => self.settings = s,
                    Err(e) => self.error = Some(e.to_string()),
                }
                self.run_due()
            }
            settings::Action::Test(llm) => match self.client_for(&llm) {
                Ok((client, _)) => {
                    self.testing = true;
                    self.test_result = None;
                    let (task, handle) = Task::perform(
                        async move {
                            client
                                .chat(ChatRequest::test())
                                .await
                                .map(|r| format!("{} replied: {}", r.model, r.text.trim()))
                                .map_err(|e| e.to_string())
                        },
                        Message::Tested,
                    )
                    .abortable();
                    self.key_requests.push(handle);
                    task
                }
                Err(e) => {
                    self.test_result = Some(Err(e));
                    Task::none()
                }
            },
            settings::Action::ListModels(llm) => match self.client_for(&llm) {
                Ok((client, _)) => {
                    self.listing_models = true;
                    self.models = None;
                    let (task, handle) = Task::perform(
                        async move { client.list_models().await.map_err(|e| e.to_string()) },
                        Message::Models,
                    )
                    .abortable();
                    self.key_requests.push(handle);
                    task
                }
                Err(e) => {
                    self.models = Some(Err(e));
                    Task::none()
                }
            },
            settings::Action::SetKey { api_key, password } => {
                self.key_message = None;
                blocking(
                    move || vault::save(&key_path, &api_key, &password).map(|()| api_key),
                    Message::KeySaved,
                )
            }
            settings::Action::ChangePassword { old, new } => {
                self.key_message = None;
                blocking(
                    move || vault::change_password(&key_path, &old, &new),
                    Message::PasswordChanged,
                )
            }
            settings::Action::RemoveKey => {
                match vault::remove(&key_path) {
                    Ok(()) => {
                        self.key_exists = false;
                        self.api_key = None;
                        self.key_message = Some(Ok("API key removed.".into()));
                    }
                    Err(e) => self.key_message = Some(Err(e.to_string())),
                }
                Task::none()
            }
            settings::Action::Lock => {
                self.lock();
                Task::none()
            }
            settings::Action::Unlock(password) => self.unlock(password),
        }
    }

    fn unlock(&mut self, password: SecretString) -> Task<Message> {
        let path = vault::key_path(&self.data_dir);
        blocking(move || vault::unlock(&path, &password), Message::Unlocked)
    }

    // ---- view ----

    fn view(&self) -> Element<'_, Message> {
        if self.unlocking {
            return self.unlock_view.view().map(Message::Unlock);
        }

        let today = self.today();
        let bar = top_bar::view(top_bar::Bar {
            tab: self.tab,
            selected: self.selected,
            today,
            schedule: &self.settings.schedule,
            quick_add: &self.quick_add,
            lock: match self.key_state() {
                KeyState::NoKey => None,
                KeyState::Locked => Some(LockIcon::Locked),
                KeyState::Unlocked => Some(LockIcon::Unlocked),
            },
            week_badge: self.week_badge,
        })
        .map(Message::Top);

        let content: Element<'_, Message> = match self.tab {
            Tab::Today => self.day_view.view().map(Message::Day),
            Tab::Week => self.week_view.view().map(Message::Week),
            Tab::Month => self.month_view.view().map(Message::Month),
        };

        let mut main = column![bar, container(content).width(Fill).height(Fill)];
        if let Some(n) = &self.notice {
            let mut bar = row![text(n.as_str()).width(Fill).style(theme::muted)]
                .spacing(theme::SPACING)
                .align_y(iced::Center);
            if n.starts_with("Deleted") && !self.undo.is_empty() {
                bar = bar.push(
                    button(text("Undo"))
                        .style(theme::secondary_button)
                        .on_press(Message::Undo),
                );
            }
            bar = bar.push(
                button(text("Dismiss"))
                    .style(theme::text_button)
                    .on_press(Message::DismissNotice),
            );
            main = main.push(container(bar).padding([8.0, theme::SPACING * 1.5]));
        }
        if let Some(e) = &self.error {
            main = main.push(
                container(
                    row![
                        text(e.as_str()).width(Fill).style(theme::danger),
                        button(text("Dismiss"))
                            .style(theme::text_button)
                            .on_press(Message::DismissError),
                    ]
                    .spacing(theme::SPACING),
                )
                .padding(theme::SPACING),
            );
        }
        let base: Element<'_, Message> = main.height(Fill).into();

        if !self.settings_open {
            return base;
        }
        let panel = container(self.settings_view.view().map(Message::Settings))
            .max_width(760)
            .height(Fill)
            .style(theme::overlay_panel);
        stack![
            base,
            opaque(
                mouse_area(
                    container(opaque(panel))
                        .padding([32, 24])
                        .center_x(Fill)
                        .height(Fill)
                        .style(theme::backdrop)
                )
                .on_press(Message::CloseSettings)
            )
        ]
        .into()
    }
}

/// The shortcut for a key press that no widget captured (PLAN.md 4.4).
fn shortcut_for(key: Key<&str>, modifiers: keyboard::Modifiers) -> Option<Shortcut> {
    if modifiers.command() {
        return match key {
            Key::Character("1") => Some(Shortcut::Tab(Tab::Today)),
            Key::Character("2") => Some(Shortcut::Tab(Tab::Week)),
            Key::Character("3") => Some(Shortcut::Tab(Tab::Month)),
            Key::Character(",") => Some(Shortcut::Settings),
            Key::Character("n") => Some(Shortcut::FocusQuickAdd),
            Key::Character("z") if !modifiers.shift() => Some(Shortcut::Undo),
            _ => None,
        };
    }
    if modifiers.alt() || modifiers.control() || modifiers.logo() {
        return None;
    }
    match key {
        Key::Character("1") => Some(Shortcut::Tab(Tab::Today)),
        Key::Character("2") => Some(Shortcut::Tab(Tab::Week)),
        Key::Character("3") => Some(Shortcut::Tab(Tab::Month)),
        Key::Character("t" | "T") => Some(Shortcut::Today),
        Key::Character("n" | "N") => Some(Shortcut::FocusQuickAdd),
        Key::Named(Named::ArrowLeft) => Some(Shortcut::Prev),
        Key::Named(Named::ArrowRight) => Some(Shortcut::Next),
        Key::Named(Named::ArrowUp) => Some(Shortcut::Nav(NavKey::Up)),
        Key::Named(Named::ArrowDown) => Some(Shortcut::Nav(NavKey::Down)),
        Key::Named(Named::Space) => Some(Shortcut::Nav(NavKey::Toggle)),
        Key::Named(Named::Enter) => Some(Shortcut::Nav(NavKey::Open)),
        Key::Named(Named::Escape) => Some(Shortcut::Escape),
        _ => None,
    }
}

/// Dev aid: `FORTNIGHT_THEME=light|dark` overrides the system theme (for
/// screenshots of both).
fn forced_theme() -> Option<bool> {
    match std::env::var("FORTNIGHT_THEME").as_deref() {
        Ok("dark") => Some(true),
        Ok("light") => Some(false),
        _ => None,
    }
}

/// Runs blocking work (Argon2) off the UI thread.
fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, VaultError> + Send + 'static,
    done: fn(Result<T, VaultError>) -> Message,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(work)
                .await
                .unwrap_or_else(|e| Err(VaultError::Io(e.to_string())))
        },
        done,
    )
}

/// `--data-dir <path>` or `--data-dir=<path>`.
fn data_dir_flag() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--data-dir" {
            return args.next().map(PathBuf::from);
        }
        if let Some(v) = arg.to_str().and_then(|s| s.strip_prefix("--data-dir=")) {
            return Some(PathBuf::from(v));
        }
    }
    None
}

/// Dev aid: with `FORTNIGHT_SCREENSHOT=<path>`, the app screenshots its own
/// window after two seconds, writes `<width> <height>\n` + raw RGBA to
/// `<path>`, and exits. Lets a headless agent check what renders.
fn shoot_after_delay() -> Task<Message> {
    if std::env::var_os("FORTNIGHT_SCREENSHOT").is_none() {
        return Task::none();
    }
    Task::perform(tokio::time::sleep(Duration::from_secs(2)), |_| {
        Message::Shoot
    })
}

fn save_screenshot(shot: iced::window::Screenshot) -> Task<Message> {
    if let Some(path) = std::env::var_os("FORTNIGHT_SCREENSHOT") {
        let mut bytes = format!("{} {}\n", shot.size.width, shot.size.height).into_bytes();
        bytes.extend_from_slice(&shot.rgba);
        if let Err(e) = std::fs::write(&path, bytes) {
            eprintln!("screenshot: {e}");
        }
    }
    iced::exit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyboard::Modifiers;

    #[test]
    fn shortcuts_follow_plan_4_4() {
        let none = Modifiers::empty();
        let cmd = Modifiers::COMMAND;
        let key = |k: Key<&'static str>, m| shortcut_for(k, m);
        assert_eq!(
            key(Key::Character("2"), none),
            Some(Shortcut::Tab(Tab::Week))
        );
        assert_eq!(
            key(Key::Character("3"), cmd),
            Some(Shortcut::Tab(Tab::Month))
        );
        assert_eq!(key(Key::Character("t"), none), Some(Shortcut::Today));
        assert_eq!(key(Key::Character("n"), cmd), Some(Shortcut::FocusQuickAdd));
        assert_eq!(key(Key::Character(","), cmd), Some(Shortcut::Settings));
        assert_eq!(key(Key::Character("z"), cmd), Some(Shortcut::Undo));
        assert_eq!(key(Key::Character("z"), cmd | Modifiers::SHIFT), None);
        assert_eq!(
            key(Key::Named(Named::ArrowLeft), none),
            Some(Shortcut::Prev)
        );
        assert_eq!(
            key(Key::Named(Named::Space), none),
            Some(Shortcut::Nav(NavKey::Toggle))
        );
        assert_eq!(
            key(Key::Named(Named::Enter), none),
            Some(Shortcut::Nav(NavKey::Open))
        );
        assert_eq!(key(Key::Named(Named::Escape), none), Some(Shortcut::Escape));
        assert_eq!(key(Key::Character("x"), none), None);
        assert_eq!(key(Key::Character("t"), Modifiers::ALT), None);
    }
}
