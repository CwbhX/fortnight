//! Fortnight: the application shell. Owns all state, carries out the actions
//! the views return, talks to the store, the key vault and the LLM, and runs
//! the minute tick that generates due reports.
//!
//! Layout (PLAN.md 4.2): the top bar (tabs, period navigation over one
//! shared selected date, lock, gear, quick-add), then the active tab. Settings
//! opens over the current view.

use fortnight_app::theme;
use fortnight_app::top_bar::{self, LockIcon, Tab};
use fortnight_app::views::day::{self, DayData, DayView, LogLine};
use fortnight_app::views::month::{self, MonthData, MonthView};
use fortnight_app::views::report_panel::{self, ReportPanelData, ReportSlot};
use fortnight_app::views::settings::{
    self, KeyState, SettingsData, SettingsView, UnlockAction, UnlockMessage, UnlockView,
};
use fortnight_app::views::week::{self, WeekData, WeekView};
use fortnight_core::datadir;
use fortnight_core::domain::{
    Event, LlmSettings, ReportKey, ReportKind, Settings, Task as CoreTask, TaskId, TaskStatus,
    WeekRef,
};
use fortnight_core::llm::{
    self, ChatEvent, ChatRequest, ChatResponse, LlmClient, LlmConfig, LlmError,
};
use fortnight_core::report::{self, Prepared};
use fortnight_core::sqlite::SqliteStore;
use fortnight_core::store::Store;
use fortnight_core::vault::{self, VaultError};
use fortnight_core::week::{due_reports, week_containing, week_label};
use iced::keyboard::{self, Key};
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
    Unlocked(Result<SecretString, VaultError>),
    KeySaved(Result<SecretString, VaultError>),
    PasswordChanged(Result<(), VaultError>),
    DismissError,
    Shoot,
    Shot(iced::window::Screenshot),
}

/// A report being generated: what was asked, and the text so far.
struct Generation {
    prepared: Prepared,
    text: String,
    requested_model: String,
    host: Option<String>,
}

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
                if !modifiers.command() {
                    return None;
                }
                match key.as_ref() {
                    Key::Character("1") => Some(Message::Top(top_bar::Message::Tab(Tab::Today))),
                    Key::Character("2") => Some(Message::Top(top_bar::Message::Tab(Tab::Week))),
                    Key::Character("3") => Some(Message::Top(top_bar::Message::Tab(Tab::Month))),
                    Key::Character(",") => Some(Message::Top(top_bar::Message::Settings)),
                    _ => None,
                }
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
    fn generate(&mut self, kind: ReportKind, week: WeekRef) -> Task<Message> {
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
        self.generating.insert(
            key,
            Generation {
                prepared,
                text: String::new(),
                requested_model: client.model().to_string(),
                host,
            },
        );
        self.last_attempt.insert(key, Timestamp::now());
        Task::run(stream, move |item| Message::Stream(key, item))
            .chain(Task::done(Message::StreamEnded(key)))
    }

    /// Stores the finished (or failed) report and forgets the generation.
    fn finish(&mut self, key: ReportKey, result: Result<ChatResponse, LlmError>) {
        let Some(g) = self.generating.remove(&key) else {
            return;
        };
        let new = report::finish(g.prepared, result, &g.requested_model, g.host);
        if let Err(e) = self.store.insert_report(new) {
            self.error = Some(e.to_string());
        }
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
                tasks.push(self.generate(job.kind, job.week));
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
            Message::Stream(key, item) => {
                match item {
                    Ok(ChatEvent::Delta(d)) => {
                        if let Some(g) = self.generating.get_mut(&key) {
                            g.text.push_str(&d);
                        }
                    }
                    Ok(ChatEvent::Done { model }) => {
                        let text = self
                            .generating
                            .get(&key)
                            .map(|g| g.text.clone())
                            .unwrap_or_default();
                        self.finish(key, Ok(ChatResponse { text, model }));
                    }
                    Err(e) => self.finish(key, Err(e)),
                }
                Task::none()
            }
            Message::StreamEnded(key) => {
                // Only still generating if the stream ended without `Done`.
                self.finish(key, Err(LlmError::Truncated));
                Task::none()
            }
            Message::Tested(result) => {
                self.testing = false;
                self.test_result = Some(result);
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
                        Ok(_) => self.quick_add.clear(),
                        Err(e) => self.error = Some(e.to_string()),
                    }
                    self.reload_tasks();
                }
                Task::none()
            }
        }
    }

    fn set_status(&mut self, id: TaskId, status: TaskStatus) {
        if let Err(e) = self.store.set_status(id, status) {
            self.error = Some(e.to_string());
        }
        self.reload_tasks();
    }

    fn day_action(&mut self, action: day::Action) -> Task<Message> {
        let result = match action {
            day::Action::None => return Task::none(),
            day::Action::SetStatus(id, status) => self.store.set_status(id, status),
            day::Action::Update(task) => self.store.update_task(&task),
            day::Action::Delete(id) => self.store.delete_task(id).map(|_| ()),
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
            report_panel::Action::Generate(kind, week) => self.generate(kind, week),
            report_panel::Action::SaveEdit(id, text) => {
                if let Err(e) = self.store.set_report_edit(id, Some(&text)) {
                    self.error = Some(e.to_string());
                }
                Task::none()
            }
            report_panel::Action::Copy(text) => iced::clipboard::write(text),
            report_panel::Action::Export(_id, _format) => {
                // STEP 1: the export dialog is wired in step 3.
                Task::none()
            }
        }
    }

    fn lock(&mut self) {
        self.api_key = None; // dropping the SecretString zeroizes it
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
                    Task::perform(
                        async move {
                            client
                                .chat(ChatRequest::test())
                                .await
                                .map(|r| format!("{} replied: {}", r.model, r.text.trim()))
                                .map_err(|e| e.to_string())
                        },
                        Message::Tested,
                    )
                }
                Err(e) => {
                    self.test_result = Some(Err(e));
                    Task::none()
                }
            },
            settings::Action::ListModels(_llm) => {
                // STEP 1: wired in step 3.
                Task::none()
            }
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
            week_badge: false,
        })
        .map(Message::Top);

        let content: Element<'_, Message> = match self.tab {
            Tab::Today => self.day_view.view().map(Message::Day),
            Tab::Week => self.week_view.view().map(Message::Week),
            Tab::Month => self.month_view.view().map(Message::Month),
        };

        let mut main = column![bar, container(content).width(Fill).height(Fill)];
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
