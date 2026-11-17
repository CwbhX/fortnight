//! Fortnight: the application shell. Owns all state, carries out the actions
//! the views return, talks to the store, the key vault and the LLM, and runs
//! the minute tick that generates due reports.
//!
//! Layout (PLAN.md 4.2): the top bar (tabs, period navigation over one
//! shared selected date, lock, gear), then the active tab. Settings opens
//! over the current view.
//!
//! Also here: undo (Ctrl/Cmd+Z, silent: the last add, status change, edit
//! or delete), the keyboard shortcuts of PLAN.md 4.4, Lock and the unlock
//! prompt (from the lock icon; the app never asks at launch), the data-folder
//! lock file and the model list.
//!
//! The Week tab's Work summary and the Today tab's Recommended tasks update
//! on their own: when their tab is shown, when the shown week or day
//! changes and on the minute tick while it is showing, each report whose
//! facts (or prompt) changed since its newest version is generated once.
//! The summary is also kept up to date in the background from its scheduled
//! day and time. An edited summary is never regenerated.

use fortnight_app::app_icon;
use fortnight_app::theme;
use fortnight_app::top_bar::{self, LockIcon, Tab};
use fortnight_app::views::NavKey;
use fortnight_app::views::day::{self, DayData, DayView, LogLine};
use fortnight_app::views::month::{self, MonthData, MonthView};
use fortnight_app::views::report_panel::{self, ReportPanelData, ReportSlot};
use fortnight_app::views::settings::{
    self, KeyState, SettingsData, SettingsView, UnlockAction, UnlockMessage, UnlockView,
};
use fortnight_app::views::task_form;
use fortnight_app::views::week::{self, WeekData, WeekView};
use fortnight_core::datadir::{self, DataLock};
use fortnight_core::decision::DecisionClient;
use fortnight_core::domain::{
    DeletedTask, Event, EventId, LlmSettings, Priority, Protocol, Report, ReportKey, ReportKind,
    ReportStatus, Settings, Task as CoreTask, TaskId, TaskStatus, WeekRef,
};
use fortnight_core::fallback::{self, Endpoint, PlanRoute};
use fortnight_core::llm::{
    self, ChatEvent, ChatRequest, ChatResponse, LlmClient, LlmConfig, LlmError,
};
use fortnight_core::ollama::{self, ModelKind, OllamaModel};
use fortnight_core::report::{self, Prepared};
use fortnight_core::secure;
use fortnight_core::sqlite::SqliteStore;
use fortnight_core::store::Store;
use fortnight_core::vault::{self, VaultError};
use fortnight_core::week::{add_months, due_reports, week_containing};
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

/// A failed automatic generation is retried after this long, doubling after
/// each further failure in a row, up to `RETRY_MAX_MINUTES`.
const RETRY_AFTER_MINUTES: i64 = 10;
const RETRY_MAX_MINUTES: i64 = 24 * 60;

/// After a change on the Week tab, the Work summary waits this long for
/// more changes (each one restarts the wait) before it is redone.
const SUMMARY_WAIT_SECS: i64 = 30;

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
    /// The task dialog (the Week tab's `+`, or a click on a card).
    Dialog(day::Message),
    Tick,
    /// Checks whether the Work summary's wait after an edit is over.
    SummaryWait,
    /// While a day's notes have unsaved typing: save them once it pauses.
    NotesTick,
    /// A later day's recommended tasks came back: the day, the facts they
    /// were worked out from, and the reply.
    DayRecs(Date, String, Result<ChatResponse, LlmError>),
    ThemeMode(iced::theme::Mode),
    /// A piece of a streamed report.
    Stream(ReportKey, u64, Result<ChatEvent, LlmError>),
    /// The stream for this report ended (after `Done` or an error, or early).
    StreamEnded(ReportKey, u64),
    Tested(Result<String, String>),
    Models(Result<Vec<OllamaModel>, String>),
    /// The local Ollama check: its models, or why it didn't answer.
    OllamaDetected(Result<Vec<OllamaModel>, String>),
    /// Typing in a new task's dialog paused: ask the decision model for a
    /// priority, if this is still the latest edit (`suggest_seq`).
    SuggestPriority(u64),
    /// The left mouse button went up anywhere: ends a card drag.
    PointerUp,
    PrioritySuggested(u64, Option<Priority>),
    /// A keyboard shortcut (applied only where it makes sense).
    Key(Shortcut),
    DismissNotice,
    /// The main key and the decision model's key, each if it exists and
    /// opened with the password.
    Unlocked(Result<(Option<SecretString>, Option<SecretString>), VaultError>),
    KeySaved(Result<SavedKey, VaultError>),
    DismissError,
    Shoot,
    Shot(iced::window::Screenshot),
    /// The window opened: show the app icon on it.
    WindowOpened,
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
    /// Where the request went (recommended tasks: the decision model, if
    /// any, falling back to the main endpoint), for the report's model and
    /// host.
    route: PlanRoute,
    /// Started by the schedule: badge the Week tab when it is ready, and
    /// never aborted for being out of view.
    scheduled: bool,
    /// Aborts the request (Lock stops requests that hold the key).
    abort: iced::task::Handle,
    id: u64,
}

/// One change that Ctrl/Cmd+Z can take back.
#[derive(Debug, Clone)]
enum Undo {
    /// A task was added: delete it (only while it has no other history).
    Added(TaskId),
    /// A task's status or fields changed. Undo puts the task back exactly as
    /// it was (including `completed_at`) and drops the events the change
    /// wrote (ids above `last_event`), keeping log lines.
    Changed {
        before: Box<CoreTask>,
        last_event: EventId,
        /// Keep the task's position as it is at undo time (a later drag
        /// stays), unless the change was a move, which set the position.
        keep_position: bool,
    },
    /// Tasks were deleted for good (one, or all of Deleted at once); put
    /// them back with their history.
    Deleted(Vec<DeletedTask>),
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
    /// The task dialog, while open.
    dialog: Option<TaskDialog>,
    /// The month the top bar's date calendar shows, while it is open.
    date_picker: Option<Date>,
    settings_open: bool,

    day_view: DayView,
    week_view: WeekView,
    month_view: MonthView,
    settings_view: SettingsView,
    unlock_view: UnlockView,
    /// The unlock prompt is showing (opened from the lock icon).
    unlocking: bool,

    key_exists: bool,
    api_key: Option<SecretString>,
    /// The decision model's own key (`vault::decision_key_path`), under the
    /// same password: unlocked and locked together with `api_key`.
    decision_key_exists: bool,
    decision_key: Option<SecretString>,

    generating: HashMap<ReportKey, Generation>,
    last_attempt: HashMap<ReportKey, Timestamp>,
    /// Failures in a row per report, for the retry backoff.
    failures: HashMap<ReportKey, u32>,
    /// When each prompt was last changed in Settings this session: versions
    /// made before then are out of date.
    prompt_changed: HashMap<ReportKind, Timestamp>,
    /// Identifies a generation, so a late message from an earlier stream for
    /// the same report can't finish a newer one.
    next_generation: u64,
    /// Bumped by each edit of a new task's title or notes, so only the
    /// latest priority suggestion lands.
    suggest_seq: u64,
    /// The open task form and its title and notes when a suggestion was
    /// last asked for: a change asks again.
    suggest_for: Option<(FormKey, String, String)>,
    /// Tasks were reloaded during this message (a change was made).
    tasks_changed: bool,
    /// The Work summary isn't redone before this: set by each change made
    /// on the Week tab (`SUMMARY_WAIT_SECS`), so a run of edits makes one
    /// new summary, not one each.
    summary_wait: Option<Timestamp>,
    testing: bool,
    test_result: Option<Result<String, String>>,
    /// Why the last key save or removal failed.
    key_message: Option<String>,
    models: Option<Result<Vec<OllamaModel>, String>>,
    /// The local Ollama (`ollama::DEFAULT_BASE_URL`): its models, or why it
    /// didn't answer. Checked at launch and when Settings opens.
    ollama: Option<Result<Vec<OllamaModel>, String>>,
    listing_models: bool,
    error: Option<String>,
    /// A short, dismissable message (the lock warning).
    notice: Option<String>,
    undo: Vec<Undo>,
    /// A report became ready while the Week tab wasn't showing.
    week_badge: bool,
    /// Recommended tasks for later days (not stored as reports): each with
    /// the facts it was worked out from, redone when those change.
    day_recs: HashMap<Date, (String, Vec<day::Recommended>)>,
    /// Later days whose recommendations are being worked out: from which
    /// facts, and the request (aborted on Lock or when the day is left).
    day_recs_pending: HashMap<Date, (String, iced::task::Handle)>,
    /// Failures in a row per later day and when the last one was, for the
    /// same retry backoff as reports.
    day_recs_failures: HashMap<Date, (u32, Timestamp)>,
    /// The priority suggestion in flight (it holds the key: Lock aborts it).
    suggest_request: Option<iced::task::Handle>,
    /// Today when the minute tick last looked: a new day moves the
    /// selection along if it was on the old one.
    last_today: Date,
    /// Values worked out from the tasks, events, reports and settings,
    /// kept until `invalidate`.
    derived: Derived,
    /// The Week tab's data as last loaded, so a streamed chunk updates
    /// only its panel slot.
    week_data: Option<WeekData>,
    /// This message changed only what a view keeps itself: no refresh,
    /// unless `must_refresh` is set too.
    local: bool,
    /// Data the views show changed (set by `invalidate` and when a request
    /// starts or stops): refresh even after a `local` message.
    must_refresh: bool,
    /// Test connection / Load models requests in flight, aborted on Lock.
    key_requests: Vec<iced::task::Handle>,
    /// The app icon last shown (`Some(true)` = dark), `None` before any.
    icon_dark: Option<bool>,
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
                // Due reports, then anything the open tab shows that is out
                // of date (e.g. recommendations from an older prompt).
                let start = ready.catch_up();
                ready.refresh();
                let tasks = Task::batch([
                    iced::system::theme().map(Message::ThemeMode),
                    operation::focus(day::QUICK_ADD_ID),
                    start,
                    detect_ollama(),
                    shoot_after_delay(),
                ]);
                let mut app = App::Ready(ready, forced_theme());
                let icon = app.sync_icon();
                (app, Task::batch([tasks, icon]))
            }
            Err(e) => (App::Fatal(e), shoot_after_delay()),
        }
    }

    fn theme(&self) -> Option<Theme> {
        // An unknown system appearance (`None`) gets Fortnight's light
        // theme, not iced's stock one.
        match self {
            App::Ready(_, dark) => Some(theme::theme_for(dark.unwrap_or(false))),
            App::Fatal(_) => None,
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let summary_wait = match self {
            App::Ready(ready, _) if ready.summary_wait.is_some() => {
                iced::time::every(Duration::from_secs(1)).map(|_| Message::SummaryWait)
            }
            _ => Subscription::none(),
        };
        let notes = match self {
            App::Ready(ready, _) if ready.day_view.notes_pending() => {
                iced::time::every(Duration::from_millis(300)).map(|_| Message::NotesTick)
            }
            _ => Subscription::none(),
        };
        Subscription::batch([
            iced::time::every(Duration::from_secs(60)).map(|_| Message::Tick),
            summary_wait,
            notes,
            iced::system::theme_changes().map(Message::ThemeMode),
            iced::window::open_events().map(|_| Message::WindowOpened),
            // Ends a card drag (Week or Today) even when the button is let
            // go outside the window.
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                    iced::mouse::Button::Left,
                )) => Some(Message::PointerUp),
                _ => None,
            }),
            keyboard::listen().filter_map(|event| {
                let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
                    return None;
                };
                shortcut_for(key.as_ref(), modifiers).map(Message::Key)
            }),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        // Any message may change the system appearance or the icon setting.
        let task = self.handle(message);
        Task::batch([task, self.sync_icon()])
    }

    /// Shows the app icon that the setting and the system appearance call
    /// for, if it is not the one showing.
    fn sync_icon(&mut self) -> Task<Message> {
        let App::Ready(ready, mode) = self else {
            return Task::none();
        };
        let dark = app_icon::is_dark(ready.settings.app_icon, *mode);
        if ready.icon_dark == Some(dark) {
            return Task::none();
        }
        ready.icon_dark = Some(dark);
        app_icon::apply(dark)
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match (&mut *self, message) {
            (_, Message::Shoot) => iced::window::latest()
                .and_then(iced::window::screenshot)
                .map(Message::Shot),
            (_, Message::Shot(shot)) => save_screenshot(shot),
            (App::Ready(ready, _), Message::WindowOpened) => {
                // A window icon set before the window existed went nowhere.
                ready.icon_dark = None;
                Task::none()
            }
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
        theme::set_accent(settings.accent);
        let tasks = store.list_tasks().map_err(|e| e.to_string())?;
        let events = store
            .events_between(Timestamp::MIN, Timestamp::MAX)
            .map_err(|e| e.to_string())?;
        let tz = TimeZone::system();
        let today = Timestamp::now().to_zoned_datetime(tz.clone()).date();
        let key_exists = vault::key_path(&dir).exists();
        let decision_key_exists = vault::decision_key_path(&dir).exists();
        let (tab, settings_open) = match std::env::var("FORTNIGHT_PAGE").as_deref() {
            Ok("week") => (Tab::Week, false),
            Ok("month") => (Tab::Month, false),
            Ok("settings") => (Tab::Today, true),
            _ => (Tab::Today, false),
        };
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
                        "This data folder is also open on {}, since {since}, or Fortnight didn't \
                         close cleanly last time. Use the folder on one machine at a time.",
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
            dialog: None,
            date_picker: None,
            settings_open,
            day_view: DayView::new(),
            week_view: WeekView::new(),
            month_view: MonthView::new(),
            settings_view: SettingsView::new(),
            unlock_view: UnlockView::new(),
            unlocking: false,
            key_exists,
            api_key: None,
            decision_key_exists,
            decision_key: None,
            generating: HashMap::new(),
            last_attempt: HashMap::new(),
            failures: HashMap::new(),
            prompt_changed: HashMap::new(),
            next_generation: 0,
            suggest_seq: 0,
            suggest_for: None,
            tasks_changed: false,
            summary_wait: None,
            testing: false,
            test_result: None,
            key_message: None,
            models: None,
            ollama: None,
            listing_models: false,
            error: None,
            notice,
            undo: Vec::new(),
            week_badge: false,
            day_recs: HashMap::new(),
            day_recs_pending: HashMap::new(),
            day_recs_failures: HashMap::new(),
            suggest_request: None,
            last_today: today,
            derived: Derived::default(),
            week_data: None,
            local: false,
            must_refresh: false,
            key_requests: Vec::new(),
            icon_dark: None,
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
            Some("no endpoint is set up in Settings.".into())
        } else if self.key_locked_for(&self.settings.llm) {
            Some("the API key is locked.".into())
        } else {
            None
        }
    }

    /// The saved key is locked and `llm` needs it. A keyless endpoint never
    /// needs it; a local one (Ollama, LM Studio) works without it and gets
    /// it only if it is unlocked.
    fn key_locked_for(&self, llm: &LlmSettings) -> bool {
        key_locked(self.key_exists, &self.api_key, llm)
    }

    /// The saved recommendations model is an Ollama decision model.
    fn decision_active(&self) -> bool {
        let d = &self.settings.decision_llm;
        d.protocol == Protocol::OllamaDecision && d.is_configured()
    }

    /// The saved decision model's client, for priority suggestions.
    fn decision_model(&self) -> Option<DecisionClient> {
        if !self.decision_active() {
            return None;
        }
        let config = fallback::decision_config(
            &self.settings.decision_llm,
            &self.settings.llm,
            self.api_key.clone(),
            self.decision_key.clone(),
        );
        // Suggestions are only worth having quickly.
        DecisionClient::new(config)
            .ok()
            .map(|c| c.with_timeout(Duration::from_secs(15)))
    }

    /// The open task form, its title and its notes: the task dialog's, or
    /// Today's inline form.
    fn open_form_text(&self) -> Option<(FormKey, String, String)> {
        if let Some(d) = &self.dialog {
            return Some((FormKey::Dialog, d.form.title.clone(), d.form.notes.text()));
        }
        if self.tab != Tab::Today {
            return None;
        }
        self.day_view
            .form_text()
            .map(|(id, title, notes)| (FormKey::Inline(id), title, notes))
    }

    /// With a decision model set up: when a task form opens or its title or
    /// notes change, ask for a suggested priority once typing pauses.
    fn maybe_suggest(&mut self) -> Task<Message> {
        if !self.decision_active() {
            return Task::none();
        }
        let now = self.open_form_text();
        if now == self.suggest_for {
            return Task::none();
        }
        let has_title = now.as_ref().is_some_and(|(_, t, _)| !t.trim().is_empty());
        self.suggest_for = now;
        self.suggest_seq += 1;
        if !has_title {
            return Task::none();
        }
        let seq = self.suggest_seq;
        Task::perform(tokio::time::sleep(Duration::from_millis(500)), move |_| {
            Message::SuggestPriority(seq)
        })
    }

    /// The client for the decision model (`Settings::decision_llm`) with
    /// these fields, and its host. It uses its own key if one is saved, and
    /// otherwise the main key only on the main endpoint's host
    /// (`fallback::decision_config`).
    fn decision_client_for(
        &self,
        llm: &LlmSettings,
    ) -> Result<(Arc<dyn LlmClient>, Option<String>), String> {
        if key_locked(self.decision_key_exists, &self.decision_key, llm) {
            return Err("The decision model's API key is locked. Unlock it first.".into());
        }
        let config = fallback::decision_config(
            llm,
            &self.settings.llm,
            self.api_key.clone(),
            self.decision_key.clone(),
        );
        let host = config.endpoint_host();
        let client = llm::connect(llm.protocol, config).map_err(|e| e.to_string())?;
        Ok((client, host))
    }

    /// The client a Settings Test or Load models button asks for.
    fn settings_client(
        &self,
        llm: &LlmSettings,
        which: settings::Which,
    ) -> Result<(Arc<dyn LlmClient>, Option<String>), String> {
        match which {
            settings::Which::Main => self.client_for(llm),
            settings::Which::Decision => self.decision_client_for(llm),
        }
    }

    fn client_for(
        &self,
        llm: &LlmSettings,
    ) -> Result<(Arc<dyn LlmClient>, Option<String>), String> {
        if self.key_locked_for(llm) {
            return Err("The API key is locked. Unlock it first.".into());
        }
        let config = LlmConfig::from_settings(llm, self.api_key.clone());
        let host = config.endpoint_host();
        let client = llm::connect(llm.protocol, config).map_err(|e| e.to_string())?;
        Ok((client, host))
    }

    // ---- data flow ----

    fn reload_tasks(&mut self) {
        self.tasks_changed = true;
        self.invalidate();
        match self.store.list_tasks() {
            Ok(tasks) => self.tasks = tasks,
            Err(e) => self.error = Some(e.to_string()),
        }
        match self.store.events_between(Timestamp::MIN, Timestamp::MAX) {
            Ok(events) => self.events = events,
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Forgets every derived value (tasks, events, reports or settings
    /// changed, or the minute ticked) and has the views refreshed.
    fn invalidate(&mut self) {
        self.derived = Derived::default();
        self.must_refresh = true;
    }

    /// Pushes fresh data into the visible tab (and Settings when open).
    fn refresh(&mut self) {
        self.must_refresh = false;
        let today = self.today();
        match self.tab {
            Tab::Today => {
                let recommended = if self.selected == today {
                    self.recommended_today()
                } else {
                    self.day_recs
                        .get(&self.selected)
                        .map(|(_, list)| list.clone())
                        .unwrap_or_default()
                };
                let recommending = if self.selected == today {
                    self.generating.contains_key(&ReportKey {
                        kind: ReportKind::Plan,
                        week_start: self.current_week().start,
                    })
                } else {
                    self.day_recs_pending.contains_key(&self.selected)
                };
                let logs = self.logs();
                self.day_view.load(DayData {
                    day: self.selected,
                    today,
                    tz: self.tz.clone(),
                    tasks: self.tasks.clone(),
                    week_start: self.settings.schedule.week_start,
                    recommended,
                    recommending,
                    logs,
                });
            }
            Tab::Week => {
                let week = self.week_of(self.selected);
                let panel = self.panel_data(week);
                let data = WeekData {
                    week,
                    today,
                    tasks: self.tasks.clone(),
                    working_days: self.settings.schedule.working_days.clone(),
                    loads: report::day_loads(&self.tasks, week, &self.settings),
                    panel,
                };
                self.week_data = Some(data.clone());
                self.week_view.load(data);
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
                decision_key_state: self.decision_key_state(),
                test_result: self.test_result.clone(),
                testing: self.testing,
                key_message: self.key_message.clone(),
                models: self.models.clone(),
                ollama: self.ollama.clone(),
                listing_models: self.listing_models,
            });
        }
    }

    /// A streamed chunk arrived for `key`: shows the text so far in its
    /// Week panel slot, if that week is showing, without a full refresh.
    fn show_streaming(&mut self, key: ReportKey) {
        if self.tab != Tab::Week {
            return;
        }
        let Some(text) = self.generating.get(&key).map(|g| g.text.clone()) else {
            return;
        };
        let Some(data) = self.week_data.as_mut() else {
            return;
        };
        if data.week.start != key.week_start {
            return;
        }
        let Some(slot) = data.panel.slots.iter_mut().find(|s| s.kind == key.kind) else {
            return;
        };
        slot.streaming = Some(text);
        self.week_view.load(data.clone());
    }

    /// The free-standing log lines (day notes), each on its local date.
    fn logs(&mut self) -> Vec<LogLine> {
        if let Some(logs) = &self.derived.logs {
            return logs.clone();
        }
        let logs: Vec<LogLine> = self
            .events
            .iter()
            .filter(|e| e.task_id.is_none())
            .filter_map(|e| {
                Some(LogLine {
                    date: self.tz.to_datetime(e.at).date(),
                    text: e.log_text()?.to_string(),
                })
            })
            .collect();
        self.derived.logs = Some(logs.clone());
        logs
    }

    /// Which reports the Week tab shows for `week`: the Summary, for the
    /// current and past weeks. (Recommended tasks moved to the Today tab.)
    fn panel_kinds(&self, week: WeekRef) -> Vec<ReportKind> {
        if week > self.current_week() {
            Vec::new()
        } else {
            vec![ReportKind::Summary]
        }
    }

    /// Today's recommended tasks: the newest ready version of the current
    /// week's Recommended tasks report that still names an open task on
    /// the Today tab's list or overdue (as the tab shows them: a chat model
    /// may not keep to that rule). An overdue task stays eligible until it
    /// is done or deleted. Falling back to an older version keeps the last
    /// good list on screen when the newest names none of them, e.g. while
    /// a new one is worked out.
    fn recommended_today(&mut self) -> Vec<day::Recommended> {
        if let Some(list) = &self.derived.recommended_today {
            return list.clone();
        }
        let today = self.today();
        let versions = self.versions(ReportKey {
            kind: ReportKind::Plan,
            week_start: self.current_week().start,
        });
        let list = versions
            .iter()
            .filter(|r| r.status == ReportStatus::Ready)
            .filter_map(|r| r.text().and_then(report::parse_recommendations))
            .map(|rec| {
                recommended(rec)
                    .into_iter()
                    .filter(|r| {
                        self.task(r.id)
                            .is_some_and(|t| t.on_today_list(today) || t.is_overdue(today))
                    })
                    .collect::<Vec<_>>()
            })
            .find(|list| !list.is_empty())
            .unwrap_or_default();
        self.derived.recommended_today = Some(list.clone());
        list
    }

    /// Every version of a report, newest first (cached until `invalidate`).
    fn versions(&mut self, key: ReportKey) -> Vec<Report> {
        if let Some(v) = self.derived.versions.get(&key) {
            return v.clone();
        }
        match self.store.list_reports(key.kind, key.week_start) {
            Ok(v) => {
                self.derived.versions.insert(key, v.clone());
                v
            }
            Err(e) => {
                self.error = Some(e.to_string());
                Vec::new()
            }
        }
    }

    /// Whether `kind` for `week` has nothing to work from, and the digest
    /// of its facts now (cached until `invalidate`).
    fn facts(&mut self, kind: ReportKind, week: WeekRef) -> (bool, String) {
        let key = ReportKey {
            kind,
            week_start: week.start,
        };
        if let Some(f) = self.derived.facts.get(&key) {
            return f.clone();
        }
        let today = self.today();
        let empty = match kind {
            ReportKind::Summary => report::summary_is_empty(&report::summary_facts(
                &self.tasks,
                &self.events,
                week,
                today,
                &self.tz,
            )),
            ReportKind::Plan => report::plan_is_empty(&report::plan_facts(
                &self.tasks,
                &self.events,
                week,
                today,
                &self.settings,
            )),
        };
        let f = (empty, self.prepare(kind, week).digest_json);
        self.derived.facts.insert(key, f.clone());
        f
    }

    /// Whether `kind` for `week` has nothing to work from, and whether it
    /// needs (re)generating (`report_stale`), given its `versions`.
    fn freshness(&mut self, kind: ReportKind, week: WeekRef, versions: &[Report]) -> (bool, bool) {
        let (empty, digest) = self.facts(kind, week);
        let stale = report_stale(
            kind,
            versions,
            &digest,
            self.prompt_changed.get(&kind).copied(),
            empty,
        );
        (empty, stale)
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
        let blocked = self.blocked();
        let mut slots = Vec::new();
        for kind in self.panel_kinds(week) {
            let key = ReportKey {
                kind,
                week_start: week.start,
            };
            let versions = self.versions(key);
            let (empty, stale) = self.freshness(kind, week, &versions);
            slots.push(ReportSlot {
                kind,
                versions,
                streaming: self.generating.get(&key).map(|g| g.text.clone()),
                blocked: blocked.clone(),
                empty,
                stale,
            });
        }
        ReportPanelData { week, slots }
    }

    /// The schedule's due reports and what the shown tab needs.
    fn catch_up(&mut self) -> Task<Message> {
        Task::batch([self.run_due(), self.update_week_reports()])
    }

    /// The date changed (past midnight): the selection follows it if it
    /// was on the old today, and later-day recommendations now past are
    /// dropped.
    fn new_day(&mut self, today: Date) {
        self.selected = follow_today(self.selected, self.last_today, today);
        self.last_today = today;
        self.day_recs.retain(|day, _| *day > today);
        self.day_recs_failures.retain(|day, _| *day > today);
        self.day_recs_pending.retain(|day, (_, handle)| {
            let keep = *day > today;
            if !keep {
                handle.abort();
            }
            keep
        });
    }

    /// Shows `tab` (and `selected`, if given): clears the Week badge when
    /// the Week tab shows, closes the date picker, and catches up what the
    /// tab shows.
    fn show(&mut self, tab: Tab, selected: Option<Date>) -> Task<Message> {
        self.tab = tab;
        if let Some(day) = selected {
            self.selected = day;
        }
        if tab == Tab::Week {
            self.week_badge = false;
        }
        self.date_picker = None;
        self.update_week_reports()
    }

    /// Aborts what browsing started and is no longer shown: generations
    /// not started by the schedule for a week other than the current or
    /// the shown one, and later-day recommendations for another day.
    fn abort_browsing(&mut self) {
        let current = self.current_week().start;
        let shown = self.week_of(self.selected).start;
        let mut aborted = false;
        self.generating.retain(|key, g| {
            let keep = keep_generation(g.scheduled, key.week_start, current, shown);
            if !keep {
                g.abort.abort();
                aborted = true;
            }
            keep
        });
        let selected = self.selected;
        self.day_recs_pending.retain(|day, (_, handle)| {
            let keep = *day == selected;
            if !keep {
                handle.abort();
                aborted = true;
            }
            keep
        });
        if aborted {
            self.must_refresh = true;
        }
    }

    /// Generates the reports the shown tab needs that are out of date, once
    /// each (none while one is in flight, and failures back off): the Week
    /// tab's Summary, and the Today tab's recommended tasks (today's, or a
    /// later day's). First aborts what browsing left behind.
    fn update_week_reports(&mut self) -> Task<Message> {
        self.abort_browsing();
        if self.blocked().is_some() {
            return Task::none();
        }
        let (week, kinds) = match self.tab {
            Tab::Week => {
                let week = self.week_of(self.selected);
                (week, self.panel_kinds(week))
            }
            Tab::Today if self.selected == self.today() => {
                (self.current_week(), vec![ReportKind::Plan])
            }
            Tab::Today => return self.update_day_recs(),
            _ => return Task::none(),
        };
        let mut tasks = Vec::new();
        for kind in kinds {
            let key = ReportKey {
                kind,
                week_start: week.start,
            };
            if kind == ReportKind::Summary && self.summary_waiting() {
                continue;
            }
            let versions = self.versions(key);
            let (empty, stale) = self.freshness(kind, week, &versions);
            if !empty && stale && !self.backing_off(key) {
                tasks.push(self.generate(kind, week, false));
            }
        }
        Task::batch(tasks)
    }

    /// Works out the recommended tasks of the later day the Today tab
    /// shows, unless they are known for its current facts or on their way.
    /// Only the open tasks planned for that day take part: overdue ones are
    /// today's business. Not stored; a failure keeps what was shown and is
    /// retried with the reports' backoff.
    fn update_day_recs(&mut self) -> Task<Message> {
        let day = self.selected;
        if self.tab != Tab::Today || day <= self.today() || self.blocked().is_some() {
            return Task::none();
        }
        let tasks: Vec<CoreTask> = self
            .tasks
            .iter()
            .filter(|t| t.status.is_open() && t.planned_for == Some(day))
            .cloned()
            .collect();
        if tasks.is_empty() {
            self.day_recs.remove(&day);
            return Task::none();
        }
        let prepared =
            report::prepare_plan(&tasks, &self.events, self.week_of(day), day, &self.settings);
        let facts = prepared.digest_json;
        if self.day_recs.get(&day).is_some_and(|(f, _)| *f == facts)
            || self
                .day_recs_pending
                .get(&day)
                .is_some_and(|(f, _)| *f == facts)
        {
            return Task::none();
        }
        let (failures, last) = self.day_recs_failures.get(&day).copied().unzip();
        if backoff_active(failures.unwrap_or(0), last, Timestamp::now()) {
            return Task::none();
        }
        let Ok(route) = self.plan_route(ReportKind::Plan) else {
            return Task::none();
        };
        let sent = facts.clone();
        let (task, handle) = Task::perform(route.client().chat(prepared.request), move |result| {
            Message::DayRecs(day, sent, result)
        })
        .abortable();
        if let Some((_, old)) = self.day_recs_pending.insert(day, (facts, handle)) {
            old.abort();
        }
        self.must_refresh = true;
        task
    }

    /// Where a report of `kind` goes: the main endpoint, and for
    /// recommended tasks the decision model first when one is set (and can
    /// be built).
    fn plan_route(&self, kind: ReportKind) -> Result<PlanRoute, String> {
        let (client, host) = self.client_for(&self.settings.llm)?;
        let decision = &self.settings.decision_llm;
        let decision = (kind == ReportKind::Plan && decision.is_configured())
            .then(|| self.decision_client_for(decision).ok())
            .flatten()
            .map(|(client, host)| Endpoint { client, host });
        Ok(PlanRoute::new(Endpoint { client, host }, decision))
    }

    /// Whether the Work summary is still waiting after a Week-tab change.
    fn summary_waiting(&self) -> bool {
        self.summary_wait.is_some_and(|at| Timestamp::now() < at)
    }

    /// A recent attempt at `key` failed and its retry wait hasn't passed
    /// (`backoff_active`).
    fn backing_off(&self, key: ReportKey) -> bool {
        backoff_active(
            self.failures.get(&key).copied().unwrap_or(0),
            self.last_attempt.get(&key).copied(),
            Timestamp::now(),
        )
    }

    /// Counts a failed attempt at `key` now, for the backoff.
    fn record_failure(&mut self, key: ReportKey) {
        *self.failures.entry(key).or_insert(0) += 1;
        self.last_attempt.insert(key, Timestamp::now());
    }

    /// Forgets every failure, so the next catch-up tries at once (the
    /// endpoint or key changed).
    fn reset_backoff(&mut self) {
        self.failures.clear();
        self.last_attempt.clear();
        self.day_recs_failures.clear();
    }

    fn key_state(&self) -> KeyState {
        key_state(self.key_exists, &self.api_key)
    }

    fn decision_key_state(&self) -> KeyState {
        key_state(self.decision_key_exists, &self.decision_key)
    }

    /// Both keys together, for the top bar's lock: locked if either saved
    /// key is.
    fn any_key_state(&self) -> KeyState {
        match (self.key_state(), self.decision_key_state()) {
            (KeyState::NoKey, KeyState::NoKey) => KeyState::NoKey,
            (KeyState::Locked, _) | (_, KeyState::Locked) => KeyState::Locked,
            _ => KeyState::Unlocked,
        }
    }

    /// Starts streaming a report unless one is already in flight. A client
    /// that can't be built counts as a failed attempt (it backs off).
    fn generate(&mut self, kind: ReportKind, week: WeekRef, scheduled: bool) -> Task<Message> {
        let key = ReportKey {
            kind,
            week_start: week.start,
        };
        if self.generating.contains_key(&key) {
            return Task::none();
        }
        let route = match self.plan_route(kind) {
            Ok(route) => route,
            Err(e) => {
                self.error = Some(e);
                self.record_failure(key);
                return Task::none();
            }
        };
        let prepared = self.prepare(kind, week);
        let stream = route.client().chat_stream(prepared.request.clone());
        self.next_generation += 1;
        let id = self.next_generation;
        let (task, abort) = Task::run(stream, move |item| Message::Stream(key, id, item))
            .chain(Task::done(Message::StreamEnded(key, id)))
            .abortable();
        self.generating.insert(
            key,
            Generation {
                prepared,
                text: String::new(),
                route,
                scheduled,
                abort,
                id,
            },
        );
        self.last_attempt.insert(key, Timestamp::now());
        self.must_refresh = true;
        task
    }

    /// Stores the finished (or failed) report and forgets the generation.
    /// A failure is counted from the stored row (an unreadable plan is
    /// stored `Failed`). A scheduled summary that became ready sets the
    /// Week badge.
    fn finish(
        &mut self,
        key: ReportKey,
        id: u64,
        result: Result<ChatResponse, LlmError>,
    ) -> Task<Message> {
        if self.generating.get(&key).is_none_or(|g| g.id != id) {
            return Task::none();
        }
        let Some(g) = self.generating.remove(&key) else {
            return Task::none();
        };
        self.invalidate();
        let (requested_model, host) = g.route.answered_by();
        let stored = report::finish(&self.store, g.prepared, result, &requested_model, host);
        let ready = match &stored {
            Ok(r) => r.status == ReportStatus::Ready,
            Err(e) => {
                self.error = Some(e.to_string());
                false
            }
        };
        if ready {
            self.failures.remove(&key);
        } else {
            *self.failures.entry(key).or_insert(0) += 1;
        }
        if stored.is_err() {
            return Task::none();
        }
        if ready && g.scheduled && key.kind == ReportKind::Summary && self.tab != Tab::Week {
            self.week_badge = true;
        }
        // Tasks may have changed while it ran: catch up straight away (a
        // failure is backing off now, so this doesn't retry it).
        if (key.kind == ReportKind::Summary && self.tab == Tab::Week)
            || (key.kind == ReportKind::Plan && self.decision_active())
        {
            return self.update_week_reports();
        }
        Task::none()
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
            return;
        };
        let result = match &last {
            Undo::Added(id) => self.undo_add(*id),
            Undo::Changed {
                before,
                last_event,
                keep_position,
            } => self.undo_change(before, *last_event, *keep_position),
            Undo::Deleted(deleted) => {
                // Put back what can be; keep the rest for another try.
                let mut left = Vec::new();
                let mut failed = None;
                for d in deleted {
                    if failed.is_none()
                        && let Err(e) = self.store.restore_task(d)
                    {
                        failed = Some(e.to_string());
                    }
                    if failed.is_some() {
                        left.push(d.clone());
                    }
                }
                match failed {
                    None => Ok(()),
                    Some(e) => {
                        self.undo.push(Undo::Deleted(left));
                        self.error = Some(format!("Could not undo: {e}"));
                        self.reload_tasks();
                        return;
                    }
                }
            }
        };
        if let Err(e) = result {
            self.error = Some(format!("Could not undo: {e}"));
            // Keep the entry (and its saved copy) for another try.
            if !matches!(last, Undo::Added(_)) {
                self.undo.push(last);
            }
        }
        self.reload_tasks();
    }

    /// Deletes a task just added, unless it has since gained history (a log
    /// line, an edit), which deleting would lose. One transaction.
    fn undo_add(&mut self, id: TaskId) -> Result<(), String> {
        match self.store.delete_new_task(id) {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err("the task has changed since you added it. Delete it instead".into()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Puts the task back as `before` (keeping its current position unless
    /// `keep_position` is false) and drops the events written since
    /// `last_event` except log lines. One transaction.
    fn undo_change(
        &mut self,
        before: &CoreTask,
        last_event: EventId,
        keep_position: bool,
    ) -> Result<(), String> {
        let target = reverted(before, self.task(before.id), keep_position);
        self.store
            .revert_task(&target, last_event)
            .map_err(|e| e.to_string())
    }

    /// Remembers `before` (the task as it is now) for undo of a change about
    /// to be made. `keep_position`: see `Undo::Changed`.
    fn snapshot(&self, id: TaskId, keep_position: bool) -> Option<Undo> {
        let before = self.task(id)?.clone();
        let last_event = self.events.iter().map(|e| e.id).max().unwrap_or(0);
        Some(Undo::Changed {
            before: Box::new(before),
            last_event,
            keep_position,
        })
    }

    fn task(&self, id: TaskId) -> Option<&CoreTask> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// The schedule (PLAN.md section 8), at launch and every minute: from
    /// its day and time the summary is kept up to date (generated when
    /// stale: out of date, or its newest attempt failed); Recommended tasks
    /// are generated when there is no ready version (otherwise only while
    /// the Today tab shows today). Waits quietly while locked or
    /// unconfigured; failures back off.
    fn run_due(&mut self) -> Task<Message> {
        if self.blocked().is_some() || self.tasks.is_empty() {
            return Task::none();
        }
        let jobs = due_reports(&self.now(), &self.settings.schedule, &[]);
        let mut tasks = Vec::new();
        for job in jobs {
            let key = ReportKey {
                kind: job.kind,
                week_start: job.week.start,
            };
            let versions = self.versions(key);
            let (empty, stale) = self.freshness(job.kind, job.week, &versions);
            let missing = !versions.iter().any(|r| r.status == ReportStatus::Ready);
            let wanted = match job.kind {
                ReportKind::Summary => stale,
                ReportKind::Plan => missing,
            };
            if wanted && !empty && !self.backing_off(key) {
                tasks.push(self.generate(job.kind, job.week, true));
            }
        }
        Task::batch(tasks)
    }

    // ---- update ----

    fn update(&mut self, message: Message) -> Task<Message> {
        // Pointer moves, timers, streams and replies change no form.
        let no_form_change = matches!(
            &message,
            Message::Day(day::Message::Moved(_) | day::Message::Enter(_) | day::Message::Leave(_))
                | Message::Week(
                    week::Message::Moved(_) | week::Message::Hover(..) | week::Message::Leave(..)
                )
                | Message::Tick
                | Message::SummaryWait
                | Message::NotesTick
                | Message::Stream(..)
                | Message::StreamEnded(..)
                | Message::DayRecs(..)
                | Message::SuggestPriority(_)
                | Message::PrioritySuggested(..)
        );
        let task = self.handle_message(message);
        if no_form_change {
            return task;
        }
        Task::batch([task, self.maybe_suggest()])
    }

    fn handle_message(&mut self, message: Message) -> Task<Message> {
        // Set by the arms whose message changes only what a view keeps.
        self.local = false;
        let task = match message {
            Message::Top(m) => self.top_bar(m),
            Message::Day(day::Message::DeleteAll) => {
                self.delete_all_deleted();
                Task::none()
            }
            Message::Day(m) => {
                // Drag bookkeeping, typing, selection, opening a form: the
                // view keeps that itself. An action that writes reloads.
                let action = self.day_view.update(m);
                self.local = true;
                self.day_action(action)
            }
            Message::PointerUp => {
                self.local = true;
                let week = match self.week_view.update(week::Message::EndDrag) {
                    week::Action::Edit(id) => self.open_edit(id),
                    _ => Task::none(),
                };
                let action = self.day_view.update(day::Message::EndDrag);
                let day = self.day_action(action);
                Task::batch([week, day])
            }
            Message::Week(m) if m.is_pointer_move() => {
                // Drag bookkeeping only: no data changed, skip the refresh.
                // A press let go without a drag is a click on the card.
                self.local = true;
                match self.week_view.update(m) {
                    week::Action::Edit(id) => self.open_edit(id),
                    _ => Task::none(),
                }
            }
            Message::Week(m) => match self.week_view.update(m) {
                week::Action::None => {
                    self.local = true;
                    Task::none()
                }
                week::Action::Move(id, day, position) => {
                    if let Some(task) = self.task(id).cloned() {
                        // Undo puts the old position back too.
                        let undo = self.snapshot(id, false);
                        let mut result = Ok(());
                        if task.day() != Some(day) {
                            // Dropping on a day plans it there; the
                            // deadline stays as it is.
                            result = self.store.update_task(&CoreTask {
                                planned_for: Some(day),
                                ..task
                            });
                        }
                        let result = result.and_then(|()| self.store.set_position(id, position));
                        match result {
                            Ok(()) => {
                                if let Some(undo) = undo {
                                    self.remember(undo);
                                }
                            }
                            Err(e) => self.error = Some(e.to_string()),
                        }
                        self.reload_tasks();
                    }
                    Task::none()
                }
                week::Action::SetStatus(id, status) => {
                    self.set_status(id, status);
                    Task::none()
                }
                week::Action::NewTask(day) => self.open_new_task(day),
                week::Action::Edit(id) => self.open_edit(id),
                week::Action::OpenDay(day) => self.show(Tab::Today, Some(day)),
                week::Action::Panel(action) => self.panel_action(action),
            },
            Message::Month(m) => match self.month_view.update(m) {
                month::Action::None => {
                    self.local = true;
                    Task::none()
                }
                month::Action::OpenDay(day) => self.show(Tab::Today, Some(day)),
                month::Action::OpenWeek(week) => self.show(Tab::Week, Some(week.start)),
            },
            Message::Settings(m) => {
                let action = self.settings_view.update(m);
                if matches!(action, settings::Action::None) {
                    self.local = true;
                }
                self.settings_action(action)
            }
            Message::CloseSettings => {
                self.hide_settings();
                Task::none()
            }
            Message::Dialog(m) => self.dialog_message(m),
            Message::Unlock(m) => match self.unlock_view.update(m) {
                UnlockAction::None => {
                    self.local = true;
                    Task::none()
                }
                UnlockAction::Skip => {
                    self.close_unlock();
                    Task::none()
                }
                UnlockAction::Unlock(password) => self.unlock(password),
            },
            Message::Tick => {
                let today = self.today();
                if today != self.last_today {
                    self.new_day(today);
                }
                // Facts depend on the date and reports may come from the
                // CLI: work everything out afresh once a minute.
                self.invalidate();
                self.catch_up()
            }
            Message::NotesTick => {
                // Saving reloads (and so refreshes); otherwise nothing changed.
                self.local = true;
                if let Some((day, notes)) = self.day_view.notes_to_save(std::time::Instant::now()) {
                    self.save_notes(day, &notes);
                }
                Task::none()
            }
            Message::DayRecs(day, facts, result) => {
                // Only the latest request for that day counts (one left
                // behind or aborted doesn't).
                if self
                    .day_recs_pending
                    .get(&day)
                    .is_none_or(|(f, _)| *f != facts)
                {
                    self.local = true;
                    return Task::none();
                }
                self.day_recs_pending.remove(&day);
                match result
                    .ok()
                    .and_then(|r| report::parse_recommendations(&r.text))
                {
                    Some(rec) => {
                        self.day_recs.insert(day, (facts, recommended(rec)));
                        self.day_recs_failures.remove(&day);
                    }
                    // Keeps what was shown; retried after the backoff.
                    None => {
                        let failures = self.day_recs_failures.get(&day).map_or(0, |f| f.0);
                        self.day_recs_failures
                            .insert(day, (failures + 1, Timestamp::now()));
                    }
                }
                // Tasks may have changed while it ran.
                self.update_day_recs()
            }
            Message::SummaryWait => {
                // Starting a generation asks for the refresh.
                self.local = true;
                if self.summary_waiting() {
                    Task::none()
                } else {
                    self.summary_wait = None;
                    self.update_week_reports()
                }
            }
            Message::Stream(key, id, item) => {
                // A finished report invalidates (and so refreshes).
                self.local = true;
                match item {
                    Ok(ChatEvent::Delta(d)) => {
                        if let Some(g) = self.generating.get_mut(&key).filter(|g| g.id == id) {
                            g.text.push_str(&d);
                            self.show_streaming(key);
                        }
                        Task::none()
                    }
                    Ok(ChatEvent::Done { model }) => {
                        let text = self
                            .generating
                            .get(&key)
                            .map(|g| g.text.clone())
                            .unwrap_or_default();
                        self.finish(key, id, Ok(ChatResponse { text, model }))
                    }
                    Err(e) => self.finish(key, id, Err(e)),
                }
            }
            Message::StreamEnded(key, id) => {
                self.local = true;
                // Only still generating if the stream ended without `Done`.
                self.finish(key, id, Err(LlmError::Truncated))
            }
            Message::Tested(result) => {
                self.testing = false;
                if !self.listing_models {
                    self.key_requests.clear();
                }
                self.test_result = Some(result);
                Task::none()
            }
            Message::SuggestPriority(seq) => {
                self.local = true;
                let Some((form, title, notes)) = self.suggest_for.clone() else {
                    return Task::none();
                };
                if seq != self.suggest_seq || title.trim().is_empty() {
                    return Task::none();
                }
                // A private task's title and notes never leave the machine.
                let task = match form {
                    FormKey::Dialog => self.dialog.as_ref().and_then(|d| d.task),
                    FormKey::Inline(id) => Some(id),
                };
                if task.and_then(|id| self.task(id)).is_some_and(|t| t.private) {
                    return Task::none();
                }
                let Some(client) = self.decision_model() else {
                    return Task::none();
                };
                let (task, handle) = Task::perform(
                    async move { client.suggest_priority(&title, &notes).await.ok() },
                    move |p| Message::PrioritySuggested(seq, p),
                )
                .abortable();
                if let Some(old) = self.suggest_request.replace(handle) {
                    old.abort();
                }
                task
            }
            Message::PrioritySuggested(seq, priority) => {
                // The forms draw the suggestion themselves.
                self.local = true;
                if seq == self.suggest_seq {
                    self.suggest_request = None;
                    match self.suggest_for.as_ref().map(|(form, _, _)| *form) {
                        Some(FormKey::Dialog) => {
                            if let Some(dialog) = self.dialog.as_mut() {
                                dialog.form.suggested = priority;
                            }
                        }
                        Some(FormKey::Inline(id)) => self.day_view.set_suggested(id, priority),
                        None => {}
                    }
                }
                Task::none()
            }
            Message::OllamaDetected(result) => {
                self.ollama = Some(result);
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
            Message::DismissNotice => {
                self.local = true;
                self.notice = None;
                Task::none()
            }
            Message::Unlocked(result) => {
                match result {
                    Ok((main, decision)) => {
                        if main.is_some() {
                            self.api_key = main;
                        }
                        if decision.is_some() {
                            self.decision_key = decision;
                        }
                        self.close_unlock();
                        self.key_message = None;
                        // What failed while locked out may work now.
                        self.reset_backoff();
                    }
                    Err(e) => self.unlock_view.set_error(e.to_string()),
                }
                self.catch_up()
            }
            Message::KeySaved(result) => {
                match result {
                    Ok(saved) => {
                        let (mine, other) = match saved.which {
                            settings::Which::Main => {
                                self.key_exists = true;
                                (&mut self.api_key, &mut self.decision_key)
                            }
                            settings::Which::Decision => {
                                self.decision_key_exists = true;
                                (&mut self.decision_key, &mut self.api_key)
                            }
                        };
                        *mine = Some(saved.key);
                        // The password opened the other key too.
                        if saved.other.is_some() {
                            *other = saved.other;
                        }
                        self.key_message = None;
                        // A new key may fix what failed.
                        self.reset_backoff();
                    }
                    // Only the check against the other key can say this.
                    Err(VaultError::WrongPassword) => {
                        self.key_message = Some(
                            "That isn't the password of your other saved key. Use the same \
                             one: it unlocks both."
                                .into(),
                        )
                    }
                    Err(e) => self.key_message = Some(e.to_string()),
                }
                self.catch_up()
            }
            Message::DismissError => {
                self.local = true;
                self.error = None;
                Task::none()
            }
            // `App::handle` takes these before they get here.
            Message::ThemeMode(_) | Message::Shoot | Message::Shot(_) | Message::WindowOpened => {
                return Task::none();
            }
        };
        let local = std::mem::take(&mut self.local);
        if !local || self.must_refresh {
            self.refresh();
        }
        if std::mem::take(&mut self.tasks_changed) {
            // The Work summary follows changes on the Week tab, once they
            // pause for `SUMMARY_WAIT_SECS`.
            if self.tab == Tab::Week {
                self.summary_wait =
                    Some(Timestamp::now() + SignedDuration::from_secs(SUMMARY_WAIT_SECS));
            }
            // A decision model is quick: recommendations follow every change.
            if self.decision_active() {
                return Task::batch([task, self.update_week_reports()]);
            }
        }
        task
    }

    fn top_bar(&mut self, message: top_bar::Message) -> Task<Message> {
        match message {
            top_bar::Message::Tab(tab) => self.show(tab, None),
            top_bar::Message::Prev => {
                self.selected = top_bar::step(self.tab, self.selected, -1);
                self.update_week_reports()
            }
            top_bar::Message::Next => {
                self.selected = top_bar::step(self.tab, self.selected, 1);
                self.update_week_reports()
            }
            top_bar::Message::Today => {
                self.selected = self.today();
                self.date_picker = None;
                self.update_week_reports()
            }
            // The date picker is drawn from `date_picker` directly.
            top_bar::Message::ToggleCalendar => {
                self.local = true;
                self.date_picker = match self.date_picker {
                    Some(_) => None,
                    None => Some(self.selected.first_of_month()),
                };
                Task::none()
            }
            top_bar::Message::CloseCalendar => {
                self.local = true;
                self.date_picker = None;
                Task::none()
            }
            top_bar::Message::CalendarMonth(step) => {
                self.local = true;
                if let Some(month) = &mut self.date_picker {
                    *month = add_months(*month, step).first_of_month();
                }
                Task::none()
            }
            top_bar::Message::PickDate(day) => {
                self.selected = day;
                self.date_picker = None;
                self.update_week_reports()
            }
            top_bar::Message::Lock => {
                if self.api_key.is_some() || self.decision_key.is_some() {
                    self.lock();
                    Task::none()
                } else {
                    self.unlocking = true;
                    UnlockView::focus().map(Message::Unlock)
                }
            }
            top_bar::Message::Settings if self.settings_open => {
                self.hide_settings();
                Task::none()
            }
            top_bar::Message::Settings => self.open_settings(),
        }
    }

    /// Opens Settings and checks for a local Ollama.
    fn open_settings(&mut self) -> Task<Message> {
        self.settings_open = true;
        detect_ollama()
    }

    /// Hides Settings, by any route: typed secrets are wiped
    /// (`SettingsView::on_hidden`); form edits are kept.
    fn hide_settings(&mut self) {
        self.settings_open = false;
        self.settings_view.on_hidden();
    }

    /// Closes the unlock prompt, by any route, wiping what was typed.
    fn close_unlock(&mut self) {
        self.unlocking = false;
        self.unlock_view.reset();
    }

    /// Opens the task dialog for a new task on `day` and focuses its title.
    fn open_new_task(&mut self, day: Date) -> Task<Message> {
        self.dialog = Some(TaskDialog {
            day: Some(day),
            form: task_form::EditForm::blank(&self.tasks, self.settings.schedule.week_start),
            task: None,
        });
        operation::focus(task_form::DIALOG_TITLE_ID)
    }

    /// Opens the task dialog to edit task `id`.
    fn open_edit(&mut self, id: TaskId) -> Task<Message> {
        if let Some(task) = self.task(id) {
            self.dialog = Some(TaskDialog {
                day: task.day(),
                form: task_form::open_form(task, &self.tasks, self.settings.schedule.week_start),
                task: Some(id),
            });
        }
        Task::none()
    }

    /// A message from the task dialog: a form edit, `Save` (add or save,
    /// then close), `Delete` (move the task to Deleted) or `Close`.
    fn dialog_message(&mut self, message: day::Message) -> Task<Message> {
        let today = self.today();
        let Some(dialog) = self.dialog.as_mut() else {
            return Task::none();
        };
        if dialog.form.handle(&message, today) {
            // The dialog is drawn from its form directly.
            self.local = true;
            return Task::none();
        }
        match (message, dialog.task) {
            (day::Message::Save, None) => {
                let day = dialog.day.unwrap_or(today);
                match dialog.form.new_task(day) {
                    Ok(new) => {
                        self.dialog = None;
                        self.add_task(new);
                    }
                    Err(e) => dialog.form.error = Some(e),
                }
            }
            (day::Message::Save, Some(id)) => {
                let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned() else {
                    self.dialog = None;
                    return Task::none();
                };
                match dialog.form.apply(&task) {
                    Ok(updated) => {
                        self.dialog = None;
                        if updated != task {
                            return self.day_action(day::Action::Update(updated));
                        }
                    }
                    Err(e) => dialog.form.error = Some(e),
                }
            }
            (day::Message::Delete, Some(id)) => {
                self.dialog = None;
                self.set_status(id, TaskStatus::Dropped);
            }
            (day::Message::Close, _) => self.dialog = None,
            _ => {}
        }
        Task::none()
    }

    /// Stores a new task, remembers it for undo, and reloads.
    fn add_task(&mut self, new: fortnight_core::domain::NewTask) {
        match self.store.add_task(new) {
            Ok(task) => self.remember(Undo::Added(task.id)),
            Err(e) => self.error = Some(e.to_string()),
        }
        self.reload_tasks();
    }

    fn set_status(&mut self, id: TaskId, status: TaskStatus) {
        let changed = self.task(id).is_some_and(|t| t.status != status);
        let undo = self.snapshot(id, true);
        match self.store.set_status(id, status) {
            Ok(()) => {
                if let (true, Some(undo)) = (changed, undo) {
                    self.remember(undo);
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.reload_tasks();
    }

    /// Saves a day's notes: its free-standing log lines are replaced by one
    /// holding `notes` (none if it is empty), stamped when the first of
    /// them was, else now on today, else noon on that day. Nothing is
    /// written when they already say the same.
    fn save_notes(&mut self, day: Date, notes: &str) {
        let old: Vec<(EventId, Timestamp, &str)> = self
            .events
            .iter()
            .filter(|e| e.task_id.is_none() && self.tz.to_datetime(e.at).date() == day)
            .filter_map(|e| Some((e.id, e.at, e.log_text()?.trim_end())))
            .collect();
        let at = match old.first() {
            Some((_, at, _)) => *at,
            None if day == self.today() => Timestamp::now(),
            None => self
                .tz
                .to_timestamp(day.at(12, 0, 0, 0))
                .unwrap_or_else(|_| Timestamp::now()),
        };
        let notes = notes.trim();
        let same = old
            .iter()
            .map(|(_, _, t)| *t)
            .collect::<Vec<_>>()
            .join("\n");
        if same == notes {
            return;
        }
        let ids: Vec<EventId> = old.iter().map(|(id, _, _)| *id).collect();
        let text = (!notes.is_empty()).then_some(notes);
        if let Err(e) = self.store.replace_logs(&ids, text, at) {
            self.error = Some(format!("Could not save the notes: {e}"));
        }
        self.reload_tasks();
    }

    /// Removes every deleted task for good (Delete all on the Today tab's
    /// Deleted section). One undo puts them all back.
    fn delete_all_deleted(&mut self) {
        let ids: Vec<TaskId> = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Dropped)
            .map(|t| t.id)
            .collect();
        let mut removed = Vec::new();
        for id in ids {
            match self.store.delete_task(id) {
                Ok(deleted) => removed.push(deleted),
                Err(e) => {
                    self.error = Some(e.to_string());
                    break;
                }
            }
        }
        if !removed.is_empty() {
            self.remember(Undo::Deleted(removed));
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
                let undo = self.snapshot(task.id, true);
                let r = self.store.update_task(&task);
                if let (Ok(()), Some(undo)) = (&r, undo) {
                    self.remember(undo);
                }
                r
            }
            day::Action::Delete(id) => match self.store.delete_task(id) {
                Ok(deleted) => {
                    self.remember(Undo::Deleted(vec![deleted]));
                    Ok(())
                }
                Err(e) => Err(e),
            },
            day::Action::Add(new) => {
                self.add_task(new);
                return Task::none();
            }
            day::Action::Reorder(positions) => self.store.set_positions(&positions),
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
            report_panel::Action::Generate(kind, week) => {
                self.failures.remove(&ReportKey {
                    kind,
                    week_start: week.start,
                });
                self.generate(kind, week, false)
            }
            report_panel::Action::SaveEdit(id, text) => {
                if let Err(e) = self.store.set_report_edit(id, text.as_deref()) {
                    self.error = Some(e.to_string());
                }
                self.invalidate();
                Task::none()
            }
            report_panel::Action::Copy(text) => iced::clipboard::write(text),
        }
    }

    /// Applies a keyboard shortcut. While the unlock prompt or Settings is
    /// showing, only the keys that close or toggle them apply.
    fn shortcut(&mut self, shortcut: Shortcut) -> Task<Message> {
        if self.unlocking {
            if shortcut == Shortcut::Escape {
                self.close_unlock();
            }
            return Task::none();
        }
        if self.settings_open {
            if matches!(shortcut, Shortcut::Escape | Shortcut::Settings) {
                self.hide_settings();
            }
            return Task::none();
        }
        if let Some(dialog) = &mut self.dialog {
            if shortcut == Shortcut::Escape {
                // An open due calendar closes first, keeping the form.
                if dialog.form.calendar.is_some() {
                    dialog.form.calendar = None;
                } else {
                    self.dialog = None;
                }
            }
            return Task::none();
        }
        match shortcut {
            Shortcut::Tab(tab) => self.top_bar(top_bar::Message::Tab(tab)),
            Shortcut::Prev => self.top_bar(top_bar::Message::Prev),
            Shortcut::Next => self.top_bar(top_bar::Message::Next),
            Shortcut::Today => self.top_bar(top_bar::Message::Today),
            // In the Week tab, the new-task dialog for the selected day;
            // elsewhere the Today tab's add-a-task input.
            Shortcut::FocusQuickAdd if self.tab == Tab::Week => self.open_new_task(self.selected),
            Shortcut::FocusQuickAdd => Task::batch([
                self.show(Tab::Today, None),
                operation::focus(day::QUICK_ADD_ID),
            ]),
            Shortcut::Settings => self.open_settings(),
            Shortcut::Undo => {
                self.undo();
                Task::none()
            }
            Shortcut::Escape => {
                if self.date_picker.is_some() {
                    self.date_picker = None;
                    Task::none()
                } else if self.tab == Tab::Today {
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
        self.decision_key = None;
        for (_, g) in self.generating.drain() {
            g.abort.abort();
        }
        for handle in self.key_requests.drain(..) {
            handle.abort();
        }
        for (_, (_, handle)) in self.day_recs_pending.drain() {
            handle.abort();
        }
        if let Some(handle) = self.suggest_request.take() {
            handle.abort();
        }
        // A suggestion already on its way is ignored.
        self.suggest_seq += 1;
        self.testing = false;
        self.listing_models = false;
        self.key_message = None;
    }

    fn settings_action(&mut self, action: settings::Action) -> Task<Message> {
        let main_path = vault::key_path(&self.data_dir);
        let decision_path = vault::decision_key_path(&self.data_dir);
        match action {
            settings::Action::None => Task::none(),
            settings::Action::Close => {
                self.hide_settings();
                Task::none()
            }
            settings::Action::Save(s) => {
                let now = Timestamp::now();
                if report::summary_prompt(&s) != report::summary_prompt(&self.settings)
                    || s.summary_instructions != self.settings.summary_instructions
                {
                    self.prompt_changed.insert(ReportKind::Summary, now);
                }
                if report::plan_prompt(&s) != report::plan_prompt(&self.settings) {
                    self.prompt_changed.insert(ReportKind::Plan, now);
                }
                match self.store.save_settings(&s) {
                    Ok(()) => {
                        theme::set_accent(s.accent);
                        // A fixed endpoint is tried again at once.
                        if s.llm != self.settings.llm
                            || s.decision_llm != self.settings.decision_llm
                        {
                            self.reset_backoff();
                        }
                        self.settings = s;
                        self.invalidate();
                        self.hide_settings();
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
                self.catch_up()
            }
            settings::Action::Test(llm, which) => match self.settings_client(&llm, which) {
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
            // Ollama (keyless, or a decision model): ask it directly, which
            // also says what each model can do.
            // A keyless server that isn't Ollama (LM Studio) answers the
            // endpoint's own model list instead.
            settings::Action::ListModels(llm, which)
                if llm.keyless || llm.protocol == Protocol::OllamaDecision =>
            {
                self.listing_models = true;
                self.models = None;
                let fallback = (llm.protocol != Protocol::OllamaDecision)
                    .then(|| self.settings_client(&llm, which).ok())
                    .flatten()
                    .map(|(client, _)| client);
                Task::perform(
                    async move {
                        match ollama::detect(&llm.base_url, Duration::from_secs(10)).await {
                            Ok(models) => Ok(models),
                            Err(e) => match fallback {
                                Some(client) => client
                                    .list_models()
                                    .await
                                    .map(chat_models)
                                    .map_err(|e| e.to_string()),
                                None => Err(e.to_string()),
                            },
                        }
                    },
                    Message::Models,
                )
            }
            settings::Action::ListModels(llm, which) => match self.settings_client(&llm, which) {
                Ok((client, _)) => {
                    self.listing_models = true;
                    self.models = None;
                    let (task, handle) = Task::perform(
                        async move {
                            client
                                .list_models()
                                .await
                                .map(chat_models)
                                .map_err(|e| e.to_string())
                        },
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
            settings::Action::SetKey {
                api_key,
                password,
                which,
            } => {
                self.key_message = None;
                let (path, other) = match which {
                    settings::Which::Main => {
                        (main_path, self.decision_key_exists.then_some(decision_path))
                    }
                    settings::Which::Decision => {
                        (decision_path, self.key_exists.then_some(main_path))
                    }
                };
                blocking(
                    move || {
                        // One password for both keys: it must open the
                        // other one, if there is one.
                        let other = other.map(|p| vault::unlock(&p, &password)).transpose()?;
                        vault::save(&path, &api_key, &password)?;
                        Ok(SavedKey {
                            which,
                            key: api_key,
                            other,
                        })
                    },
                    Message::KeySaved,
                )
            }
            settings::Action::RemoveKey(which) => {
                let (path, exists, key) = match which {
                    settings::Which::Main => (main_path, &mut self.key_exists, &mut self.api_key),
                    settings::Which::Decision => (
                        decision_path,
                        &mut self.decision_key_exists,
                        &mut self.decision_key,
                    ),
                };
                match vault::remove(&path) {
                    Ok(()) => {
                        *exists = false;
                        *key = None;
                        self.key_message = None;
                    }
                    Err(e) => self.key_message = Some(e.to_string()),
                }
                Task::none()
            }
            settings::Action::ShowUnlock => {
                self.unlocking = true;
                UnlockView::focus().map(Message::Unlock)
            }
            settings::Action::FocusPassword => operation::focus(settings::PASSWORD_INPUT_ID),
        }
    }

    /// Opens both saved keys with `password`. Fails only if it opens
    /// neither.
    fn unlock(&mut self, password: SecretString) -> Task<Message> {
        let main = self.key_exists.then(|| vault::key_path(&self.data_dir));
        let decision = self
            .decision_key_exists
            .then(|| vault::decision_key_path(&self.data_dir));
        blocking(
            move || {
                let open = |p: Option<PathBuf>| p.map(|p| vault::unlock(&p, &password)).transpose();
                match (open(main), open(decision)) {
                    (Err(e), Err(_) | Ok(None)) | (Ok(None), Err(e)) => Err(e),
                    (main, decision) => Ok((main.ok().flatten(), decision.ok().flatten())),
                }
            },
            Message::Unlocked,
        )
    }

    // ---- view ----

    fn view(&self) -> Element<'_, Message> {
        let base = self.main_view();
        if !self.unlocking {
            return base;
        }
        stack![base, opaque(self.unlock_view.view().map(Message::Unlock))].into()
    }

    fn main_view(&self) -> Element<'_, Message> {
        let today = self.today();
        let bar = top_bar::view(top_bar::Bar {
            tab: self.tab,
            selected: self.selected,
            today,
            schedule: &self.settings.schedule,
            lock: match self.any_key_state() {
                KeyState::NoKey => None,
                KeyState::Locked => Some(LockIcon::Locked),
                KeyState::Unlocked => Some(LockIcon::Unlocked),
            },
            week_badge: self.week_badge,
            calendar: self.date_picker,
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

        if let Some(dialog) = &self.dialog {
            let panel = container(
                task_form::task_dialog(&dialog.form, dialog.day, dialog.task.is_some(), today)
                    .map(Message::Dialog),
            )
            .max_width(640)
            .style(theme::overlay_panel);
            return stack![
                base,
                opaque(
                    mouse_area(
                        container(opaque(panel))
                            .padding([32, 24])
                            .center(Fill)
                            .style(theme::dialog_backdrop)
                    )
                    .on_press(Message::Dialog(day::Message::Close))
                )
            ]
            .into();
        }
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

/// The task dialog over everything: a new task for `day`, or an edit of
/// `task`.
struct TaskDialog {
    /// The day a new task is planned for; an edited task's day, for the
    /// heading.
    day: Option<Date>,
    form: task_form::EditForm,
    /// The task being edited; `None` for a new task.
    task: Option<TaskId>,
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

/// The saved key is locked (`exists` but not open) and `llm` needs it
/// (`secure::needs_saved_key`).
fn key_locked(exists: bool, key: &Option<SecretString>, llm: &LlmSettings) -> bool {
    exists && key.is_none() && secure::needs_saved_key(llm)
}

/// The tasks a recommendation names, in its order.
fn recommended(rec: report::Recommendation) -> Vec<day::Recommended> {
    rec.tasks
        .into_iter()
        .map(|t| day::Recommended { id: t.id })
        .collect()
}

/// Model ids from an endpoint's model list, as chat models.
fn chat_models(ids: Vec<String>) -> Vec<OllamaModel> {
    ids.into_iter()
        .map(|name| OllamaModel {
            name,
            kind: ModelKind::Chat,
        })
        .collect()
}

/// Values worked out from the tasks, events, reports and settings, kept
/// until `Ready::invalidate`.
#[derive(Default)]
struct Derived {
    /// Every version of a report, newest first.
    versions: HashMap<ReportKey, Vec<Report>>,
    /// Whether a report has nothing to work from, and its facts' digest.
    facts: HashMap<ReportKey, (bool, String)>,
    recommended_today: Option<Vec<day::Recommended>>,
    logs: Option<Vec<LogLine>>,
}

/// Whether a report needs (re)generating, from its `versions` (newest
/// first) and the `digest` of its facts now. Judged against the newest
/// `Ready` version: an edited one never needs it; otherwise it does when
/// the newest attempt failed, or the ready version was built from other
/// facts, an older plan prompt, or before the prompt was changed
/// (`prompt_changed`). With no versions, it does unless `empty`.
fn report_stale(
    kind: ReportKind,
    versions: &[Report],
    digest: &str,
    prompt_changed: Option<Timestamp>,
    empty: bool,
) -> bool {
    let ready = versions.iter().find(|r| r.status == ReportStatus::Ready);
    match ready {
        Some(r) if r.edited_md.is_some() => false,
        _ if versions
            .first()
            .is_some_and(|r| r.status == ReportStatus::Failed) =>
        {
            true
        }
        Some(r) => {
            // Recommendations made with an older prompt (the weekly list
            // before it became today's top three) are redone.
            (kind == ReportKind::Plan && r.prompt_version < report::PROMPT_VERSION)
                || r.digest_json != digest
                || prompt_changed.is_some_and(|at| r.created_at < at)
        }
        None => !empty,
    }
}

/// After `failures` failed attempts in a row, the last at `last_attempt`,
/// whether to wait before the next: `RETRY_AFTER_MINUTES`, doubling with
/// each further failure, up to `RETRY_MAX_MINUTES`.
fn backoff_active(failures: u32, last_attempt: Option<Timestamp>, now: Timestamp) -> bool {
    if failures == 0 {
        return false;
    }
    let wait = RETRY_AFTER_MINUTES
        .saturating_mul(1 << (failures - 1).min(10))
        .min(RETRY_MAX_MINUTES);
    last_attempt.is_some_and(|t| now.duration_since(t) < SignedDuration::from_mins(wait))
}

/// Whether a generation stays when the user browses: one the schedule
/// started, or one for the current week or the shown one.
fn keep_generation(scheduled: bool, week_start: Date, current: Date, shown: Date) -> bool {
    scheduled || week_start == current || week_start == shown
}

/// The selected date after the day changed from `last_today` to `today`:
/// a user who was on the old today moves to the new one; one who had gone
/// elsewhere stays.
fn follow_today(selected: Date, last_today: Date, today: Date) -> Date {
    if selected == last_today {
        today
    } else {
        selected
    }
}

/// What undo of an edit writes back: `before`, but with the task's
/// `current` position when `keep_position` (a later drag stays).
fn reverted(before: &CoreTask, current: Option<&CoreTask>, keep_position: bool) -> CoreTask {
    let mut task = before.clone();
    if keep_position && let Some(current) = current {
        task.position = current.position;
    }
    task
}

/// Checks for Ollama on this machine (`Message::OllamaDetected`).
fn detect_ollama() -> Task<Message> {
    Task::perform(
        async {
            ollama::detect(&ollama::local_base_url(), Duration::from_secs(2))
                .await
                .map_err(|e| e.to_string())
        },
        Message::OllamaDetected,
    )
}

/// Which task form a priority suggestion is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormKey {
    /// The task dialog (new or existing task).
    Dialog,
    /// Today's inline form for this task.
    Inline(TaskId),
}

/// A key `SetKey` saved, and the other key if the password opened it.
#[derive(Debug, Clone)]
struct SavedKey {
    which: settings::Which,
    key: SecretString,
    other: Option<SecretString>,
}

fn key_state(exists: bool, key: &Option<SecretString>) -> KeyState {
    match (exists, key.is_some()) {
        (false, _) => KeyState::NoKey,
        (true, false) => KeyState::Locked,
        (true, true) => KeyState::Unlocked,
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
    // `FORTNIGHT_SCREENSHOT_DELAY` (seconds) waits longer, e.g. for the
    // Ollama check.
    let secs = std::env::var("FORTNIGHT_SCREENSHOT_DELAY")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(2);
    Task::perform(tokio::time::sleep(Duration::from_secs(secs)), |_| {
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

    use fortnight_core::domain::NewReport;
    use jiff::civil::date;

    fn report(status: ReportStatus, digest: &str) -> Report {
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .insert_report(NewReport {
                kind: ReportKind::Summary,
                week_start: date(2026, 11, 16),
                status,
                digest_json: digest.into(),
                content_md: (status == ReportStatus::Ready).then(|| "text".into()),
                model: Some("m".into()),
                endpoint_host: None,
                prompt_version: report::PROMPT_VERSION,
                error: (status == ReportStatus::Failed).then(|| "down".into()),
            })
            .unwrap()
    }

    #[test]
    fn a_failed_newest_attempt_is_stale_unless_edited() {
        use ReportStatus::{Failed, Ready};
        let stale = |versions: &[Report], empty| {
            report_stale(ReportKind::Summary, versions, "d", None, empty)
        };
        // Nothing yet: wanted unless there is nothing to work from.
        assert!(stale(&[], false));
        assert!(!stale(&[], true));
        // Ready from the same facts: up to date; other facts: stale.
        assert!(!stale(&[report(Ready, "d")], false));
        assert!(stale(&[report(Ready, "old")], false));
        // A failure from the same facts doesn't count as up to date,
        // with or without an earlier ready version.
        assert!(stale(&[report(Failed, "d")], false));
        assert!(stale(&[report(Failed, "d"), report(Ready, "d")], false));
        // An older failure under a newer ready version is history.
        assert!(!stale(&[report(Ready, "d"), report(Failed, "d")], false));
        // An edited ready version still wins.
        let mut edited = report(Ready, "old");
        edited.edited_md = Some("mine".into());
        assert!(!stale(&[report(Failed, "d"), edited], false));
        // A prompt change after the ready version makes it stale.
        let r = report(Ready, "d");
        let later = r.created_at + SignedDuration::from_secs(1);
        assert!(report_stale(
            ReportKind::Summary,
            &[r],
            "d",
            Some(later),
            false
        ));
    }

    #[test]
    fn backoff_doubles_up_to_a_day() {
        let t0 = Timestamp::UNIX_EPOCH;
        let after = |mins| t0 + SignedDuration::from_mins(mins);
        assert!(!backoff_active(0, Some(t0), t0));
        assert!(backoff_active(1, Some(t0), after(9)));
        assert!(!backoff_active(1, Some(t0), after(10)));
        assert!(backoff_active(2, Some(t0), after(19)));
        assert!(!backoff_active(2, Some(t0), after(20)));
        assert!(backoff_active(30, Some(t0), after(24 * 60 - 1)));
        assert!(!backoff_active(30, Some(t0), after(24 * 60)));
        // No attempt on record: nothing to wait for.
        assert!(!backoff_active(3, None, t0));
    }

    #[test]
    fn browsing_keeps_only_scheduled_current_and_shown_weeks() {
        let (past, current, shown) = (date(2026, 11, 2), date(2026, 11, 16), date(2026, 11, 9));
        assert!(!keep_generation(false, past, current, shown));
        assert!(keep_generation(true, past, current, shown));
        assert!(keep_generation(false, current, current, shown));
        assert!(keep_generation(false, shown, current, shown));
    }

    #[test]
    fn the_selection_follows_a_new_day_only_from_the_old_today() {
        let (old, new) = (date(2026, 11, 17), date(2026, 11, 18));
        assert_eq!(follow_today(old, old, new), new);
        let elsewhere = date(2026, 11, 25);
        assert_eq!(follow_today(elsewhere, old, new), elsewhere);
    }

    #[test]
    fn undoing_an_edit_keeps_a_later_drag() {
        use fortnight_core::domain::NewTask;
        let store = SqliteStore::open_in_memory().unwrap();
        let mut before = store
            .add_task(NewTask {
                title: "t".into(),
                ..NewTask::default()
            })
            .unwrap();
        before.position = 1.0;
        let mut now = before.clone();
        now.title = "edited".into();
        now.position = 5.0;
        let undone = reverted(&before, Some(&now), true);
        assert_eq!(undone.title, "t");
        assert_eq!(undone.position, 5.0);
        // A move's undo puts the old position back.
        assert_eq!(reverted(&before, Some(&now), false).position, 1.0);
        // The task is gone: nothing to keep.
        assert_eq!(reverted(&before, None, true).position, 1.0);
    }

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
