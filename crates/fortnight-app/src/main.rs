//! Fortnight: the application shell. Owns all state (`Ready`), carries out
//! the actions the views return, talks to the store, the key vault and the
//! LLM, and runs the minute tick that generates due reports.
//!
//! Layout (PLAN.md 4.2): the top bar (tabs, period navigation over one
//! shared selected date, lock, gear), then the active tab. Settings opens
//! over the current view. The few animations (the open tab fading in, the
//! dialogs, the tab pill) run themselves: see `fortnight_app::motion`.
//!
//! This file holds the app plumbing (boot, theme, icon, subscriptions),
//! the `Message` enum, the `Ready` state, the message dispatch, navigation
//! and the view. The rest of the shell is in `shell/`, mostly as further
//! `impl Ready` blocks:
//! - `reports`: report generation and scheduling, and the Week tab's
//!   report panel. The Weekly Summary and the Today tab's Recommended tasks
//!   update on their own; an edited summary is never regenerated.
//! - `recommend`: today's and later days' recommended tasks, and the plan
//!   route.
//! - `keys`: the API keys, the LLM clients, Lock and unlock.
//! - `edits`: task writes and undo (Ctrl/Cmd+Z).
//! - `suggest`: priority suggestions for an open task form.
//! - `settings_panel`: the Settings panel's actions and request replies.
//! - `dialog`: the task dialog.
//! - `shortcuts`: the keyboard shortcuts of PLAN.md 4.4.
//! - `refresh`: reloading data, the derived-value cache, and pushing data
//!   into the views.
//! - `dev`: developer hooks (`--data-dir`, `FORTNIGHT_*`).

mod shell {
    pub(crate) mod dev;
    pub(crate) mod dialog;
    pub(crate) mod edits;
    pub(crate) mod keys;
    pub(crate) mod recommend;
    pub(crate) mod refresh;
    pub(crate) mod reports;
    pub(crate) mod settings_panel;
    pub(crate) mod shortcuts;
    pub(crate) mod suggest;
}

use fortnight_app::app_icon;
use fortnight_app::motion;
use fortnight_app::theme;
use fortnight_app::top_bar::{self, LockIcon, Tab};
use fortnight_app::views::day::{self, DayView};
use fortnight_app::views::month::{self, MonthView};
use fortnight_app::views::search::{self, SearchData, SearchView};
use fortnight_app::views::settings::{
    self, KeyState, SettingsView, UnlockAction, UnlockMessage, UnlockView,
};
use fortnight_app::views::shortcuts_help;
use fortnight_app::views::task_form;
use fortnight_app::views::week::{self, WeekData, WeekView};
use fortnight_core::datadir::{self, DataLock};
use fortnight_core::domain::{
    Event, Priority, ReportKey, Settings, Task as CoreTask, TaskId, WeekRef,
};
use fortnight_core::llm::{ChatEvent, ChatResponse, LlmError};
use fortnight_core::ollama::OllamaModel;
use fortnight_core::sqlite::SqliteStore;
use fortnight_core::store::Store;
use fortnight_core::vault::VaultError;
use fortnight_core::week::{add_months, week_containing};
use iced::keyboard;
use iced::widget::{
    Space, button, center, column, container, mouse_area, opaque, operation, row, stack, text,
};
use iced::{Element, Fill, Subscription, Task, Theme};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ZonedDateTime};
use secrecy::SecretString;
use shell::dev::{self, data_dir_flag, forced_theme, save_screenshot, shoot_after_delay};
use shell::edits::UndoStack;
use shell::keys::{Keys, SavedKey};
use shell::recommend::LaterDays;
use shell::refresh::Derived;
use shell::reports::Reports;
use shell::settings_panel::{detect_ollama, load_models};
use shell::shortcuts::{Shortcut, shortcut_for};
use shell::suggest::Suggestions;
use std::path::PathBuf;
use std::time::Duration;

pub fn main() -> iced::Result {
    dev::set_up_motion();
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
        .exit_on_close_request(false)
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    Top(top_bar::Message),
    Day(day::Message),
    Week(week::Message),
    Month(month::Message),
    Settings(settings::Message),
    Search(search::Message),
    CloseSearch,
    ShortcutsHelp(shortcuts_help::Message),
    CloseShortcuts,
    Unlock(UnlockMessage),
    CloseSettings,
    /// The task dialog (the Week tab's `+`, or a click on a card).
    Dialog(day::Message),
    Tick,
    /// Checks whether the Weekly Summary's wait after an edit is over.
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
    Tested(u64, Result<String, String>),
    Models(settings::Which, u64, Result<Vec<OllamaModel>, String>),
    /// The local Ollama check: its models, or why it didn't answer.
    OllamaDetected(Result<Vec<OllamaModel>, String>),
    /// Typing in a new task's dialog paused: ask the decision model for a
    /// priority, if this is still the latest edit (`Suggestions`).
    SuggestPriority(u64),
    /// The left mouse button went up anywhere: ends a card drag.
    PointerUp,
    PrioritySuggested(u64, Option<Priority>),
    /// A keyboard shortcut (applied only where it makes sense).
    Key(Shortcut),
    DismissNotice,
    /// The main key and the decision model's key, each if it exists and
    /// opened with the password.
    Unlocked(
        u64,
        Result<(Option<SecretString>, Option<SecretString>), VaultError>,
    ),
    KeySaved(u64, Result<SavedKey, VaultError>),
    DismissError,
    Shoot,
    Shot(iced::window::Screenshot),
    /// The window opened: show the app icon and apply saved size.
    WindowOpened(iced::window::Id),
    /// The window was resized: remember its size for the next launch.
    WindowResized(iced::Size),
    /// The window gained focus: check for external DB writes (e.g. CLI).
    WindowFocused,
    /// The user closed the window: flush unsaved notes, save window size, exit.
    CloseRequested,
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
    /// The task dialog, while open.
    dialog: Option<TaskDialog>,
    /// The month the top bar's date calendar shows, while it is open.
    date_picker: Option<Date>,
    settings_open: bool,
    search_open: bool,
    shortcuts_open: bool,

    day_view: DayView,
    week_view: WeekView,
    month_view: MonthView,
    settings_view: SettingsView,
    search_view: SearchView,
    unlock_view: UnlockView,
    /// The unlock prompt is showing (opened from the lock icon).
    unlocking: bool,

    /// The API keys.
    keys: Keys,

    /// Report generation and scheduling.
    reports: Reports,
    /// Priority suggestions for the open task form.
    suggest: Suggestions,
    /// Tasks were reloaded during this message (a change was made).
    tasks_changed: bool,
    testing: bool,
    test_result: Option<Result<String, String>>,
    /// Why the last key save or removal failed.
    key_message: Option<String>,
    /// True while Argon2 is encrypting a new API key.
    saving_key: bool,
    /// Result of the last Settings export action.
    export_status: Option<Result<String, String>>,
    /// Last seen SQLite `PRAGMA data_version` for detecting external CLI writes.
    last_data_version: i64,
    /// Last observed window size `(width, height)`, persisted on close.
    window_size: Option<(u32, u32)>,
    /// Saved window size has been applied once on launch.
    window_sized: bool,
    /// Each Settings block's model list.
    models: settings::PerBlock<settings::ModelList>,
    /// Each block's model-list request in flight: a newer one, and Lock,
    /// abort it.
    model_requests: settings::PerBlock<Option<iced::task::Handle>>,
    /// The local Ollama (`ollama::DEFAULT_BASE_URL`): its models, or why it
    /// didn't answer. Checked at launch and when Settings opens.
    ollama: Option<Result<Vec<OllamaModel>, String>>,
    error: Option<String>,
    /// A short, dismissable message (the lock warning).
    notice: Option<String>,
    undo: UndoStack,
    /// A report became ready while the Week tab wasn't showing.
    week_badge: bool,
    /// Recommended tasks for later days.
    later_days: LaterDays,
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
    /// Test connection requests in flight, aborted on Lock.
    key_requests: Vec<iced::task::Handle>,
    /// Sequence numbers for async key/test/model operations so cancelled or
    /// pre-Lock completions are ignored.
    unlock_seq: u64,
    key_save_seq: u64,
    test_seq: u64,
    model_seq: settings::PerBlock<u64>,
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
                // Settings open from the start (`FORTNIGHT_PAGE`) loads its
                // model lists as opening it would.
                let (models, focus) = if ready.settings_open {
                    (load_models(), Task::none())
                } else if ready.search_open {
                    (Task::none(), SearchView::focus().map(Message::Search))
                } else {
                    (Task::none(), operation::focus(day::QUICK_ADD_ID))
                };
                let tasks = Task::batch([
                    iced::system::theme().map(Message::ThemeMode),
                    focus,
                    start,
                    detect_ollama(),
                    models,
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
        // `FORTNIGHT_THEME` beats the setting; an unknown system appearance
        // gets Fortnight's light theme, not iced's stock one.
        match self {
            App::Ready(ready, system) => {
                let dark = forced_theme()
                    .unwrap_or_else(|| theme::is_dark(ready.settings.appearance, *system));
                Some(theme::theme_for(dark))
            }
            App::Fatal(_) => None,
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let summary_wait = match self {
            App::Ready(ready, _) if ready.reports.has_summary_wait() => {
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
            iced::window::events().filter_map(|(id, event)| match event {
                iced::window::Event::Opened { .. } => Some(Message::WindowOpened(id)),
                iced::window::Event::Resized(size) => Some(Message::WindowResized(size)),
                iced::window::Event::Focused => Some(Message::WindowFocused),
                iced::window::Event::CloseRequested => Some(Message::CloseRequested),
                _ => None,
            }),
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
            (App::Fatal(_), Message::CloseRequested) => iced::exit(),
            (App::Ready(ready, _), Message::WindowOpened(id)) => {
                // A window icon set before the window existed went nowhere.
                ready.icon_dark = None;
                if !ready.window_sized {
                    ready.window_sized = true;
                    if let Some((w, h)) = ready.settings.window_size {
                        let w = w.max(640) as f32;
                        let h = h.max(480) as f32;
                        return iced::window::resize(id, iced::Size::new(w, h));
                    }
                }
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
        theme::set_today_width(settings.today_width);
        let tasks = store.list_tasks().map_err(|e| e.to_string())?;
        let events = store
            .events_between(Timestamp::MIN, Timestamp::MAX)
            .map_err(|e| e.to_string())?;
        let last_data_version = store.data_version().unwrap_or(0);
        let window_size = settings.window_size;
        let tz = TimeZone::system();
        let today = Timestamp::now().to_zoned_datetime(tz.clone()).date();
        let keys = Keys::open(&dir);
        let (tab, settings_open, search_open) = dev::start_page();
        let mut week_view = WeekView::new();
        week_view.set_summary_open(dev::open_summary());
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
            search_open,
            shortcuts_open: false,
            day_view: DayView::new(),
            week_view,
            month_view: MonthView::new(),
            settings_view: SettingsView::new(),
            search_view: SearchView::new(),
            unlock_view: UnlockView::new(),
            unlocking: false,
            keys,
            reports: Reports::default(),
            suggest: Suggestions::default(),
            tasks_changed: false,
            testing: false,
            test_result: None,
            key_message: None,
            saving_key: false,
            export_status: None,
            last_data_version,
            window_size,
            window_sized: false,
            models: settings::PerBlock::default(),
            model_requests: settings::PerBlock::default(),
            ollama: None,
            error: None,
            notice,
            undo: UndoStack::default(),
            week_badge: false,
            later_days: LaterDays::default(),
            last_today: today,
            derived: Derived::default(),
            week_data: None,
            local: false,
            must_refresh: false,
            key_requests: Vec::new(),
            unlock_seq: 0,
            key_save_seq: 0,
            test_seq: 0,
            model_seq: settings::PerBlock::default(),
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

    /// The date changed (past midnight): the selection follows it if it
    /// was on the old today, and later-day recommendations now past are
    /// dropped.
    fn new_day(&mut self, today: Date) {
        self.selected = follow_today(self.selected, self.last_today, today);
        self.last_today = today;
        self.later_days.forget_past(today);
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

    fn task(&self, id: TaskId) -> Option<&CoreTask> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Checks SQLite's `PRAGMA data_version` and reloads tasks, events and
    /// settings if another process (such as `fortnight-cli`) modified the DB.
    fn sync_external_changes(&mut self) -> bool {
        let Ok(version) = self.store.data_version() else {
            return false;
        };
        if version == self.last_data_version {
            return false;
        }
        self.last_data_version = version;
        if let Ok(settings) = self.store.load_settings() {
            theme::set_accent(settings.accent);
            theme::set_today_width(settings.today_width);
            self.settings = settings;
        }
        self.reload_tasks();
        true
    }

    /// Opens the Quick Search & Project Filter overlay and focuses its input.
    pub(crate) fn open_search(&mut self) -> Task<Message> {
        self.date_picker = None;
        self.shortcuts_open = false;
        self.search_open = true;
        self.search_view.reset();
        let today = self.today();
        let logs = self.events_as_logs();
        self.search_view.load(SearchData {
            tasks: self.tasks.clone(),
            logs,
            today,
        });
        SearchView::focus().map(Message::Search)
    }

    /// Closes Quick Search and clears its query.
    pub(crate) fn close_search(&mut self) {
        self.search_open = false;
        self.search_view.reset();
    }

    /// Carries out what Quick Search asked for.
    pub(crate) fn search_action(&mut self, action: search::Action) -> Task<Message> {
        match action {
            search::Action::None => {
                self.local = true;
                Task::none()
            }
            search::Action::Close => {
                self.close_search();
                Task::none()
            }
            search::Action::OpenTask { id, day } => {
                self.close_search();
                self.day_view.select_and_open(id);
                self.show(Tab::Today, Some(day))
            }
            search::Action::OpenDay(day) => {
                self.close_search();
                self.show(Tab::Today, Some(day))
            }
        }
    }

    fn events_as_logs(&self) -> Vec<day::LogLine> {
        self.events
            .iter()
            .filter(|e| e.task_id.is_none())
            .filter_map(|e| {
                Some(day::LogLine {
                    date: self.tz.to_datetime(e.at).date(),
                    text: e.log_text()?.to_string(),
                })
            })
            .collect()
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
                | Message::WindowResized(_)
                | Message::WindowFocused
                | Message::CloseRequested
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
                let action = self.week_view.update(week::Message::EndDrag);
                let week = self.week_action(action);
                let action = self.day_view.update(day::Message::EndDrag);
                let day = self.day_action(action);
                Task::batch([week, day])
            }
            Message::Week(m) => {
                let action = self.week_view.update(m);
                self.week_action(action)
            }
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
            Message::Search(m) => {
                let action = self.search_view.update(m);
                self.search_action(action)
            }
            Message::CloseSearch => {
                self.close_search();
                Task::none()
            }
            Message::ShortcutsHelp(shortcuts_help::Message::Close) | Message::CloseShortcuts => {
                self.shortcuts_open = false;
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
                self.sync_external_changes();
                // Facts depend on the date and reports may come from the
                // CLI: work everything out afresh once a minute.
                self.invalidate();
                self.catch_up()
            }
            Message::WindowResized(size) => {
                self.local = true;
                let w = size.width.round() as u32;
                let h = size.height.round() as u32;
                if w >= 640 && h >= 480 {
                    self.window_size = Some((w, h));
                }
                Task::none()
            }
            Message::WindowFocused => {
                if self.sync_external_changes() {
                    self.catch_up()
                } else {
                    self.local = true;
                    Task::none()
                }
            }
            Message::CloseRequested => {
                // Flush any pending day notes before quitting so last-second
                // typing is never lost.
                for (day, text) in self.day_view.flush_notes() {
                    let _ = self.save_notes(day, &text);
                }
                if self.window_size != self.settings.window_size {
                    let mut s = self.settings.clone();
                    s.window_size = self.window_size;
                    let _ = self.store.save_settings(&s);
                }
                return iced::exit();
            }
            Message::NotesTick => {
                // Saving reloads (and so refreshes); otherwise nothing changed.
                self.local = true;
                if let Some((day, notes)) = self.day_view.notes_to_save(std::time::Instant::now())
                    && !self.save_notes(day, &notes)
                {
                    // Kept for another try.
                    self.day_view.notes_unsaved(day, notes);
                }
                Task::none()
            }
            Message::DayRecs(day, facts, result) => {
                let Some(task) = self.day_recs_done(day, facts, result) else {
                    self.local = true;
                    return Task::none();
                };
                task
            }
            Message::SummaryWait => {
                // Starting a generation asks for the refresh.
                self.local = true;
                self.summary_wait_over()
            }
            Message::Stream(key, id, item) => {
                // A finished report invalidates (and so refreshes).
                self.local = true;
                self.stream(key, id, item)
            }
            Message::StreamEnded(key, id) => {
                self.local = true;
                self.stream_ended(key, id)
            }
            Message::Tested(seq, result) => self.tested(seq, result),
            Message::SuggestPriority(seq) => {
                self.local = true;
                let Some(task) = self.suggest_priority(seq) else {
                    return Task::none();
                };
                task
            }
            Message::PrioritySuggested(seq, priority) => {
                // The forms draw the suggestion themselves.
                self.local = true;
                self.priority_suggested(seq, priority);
                Task::none()
            }
            Message::OllamaDetected(result) => self.ollama_detected(result),
            Message::Models(which, seq, result) => self.models_listed(which, seq, result),
            Message::Key(shortcut) => self.shortcut(shortcut),
            Message::DismissNotice => {
                self.local = true;
                self.notice = None;
                Task::none()
            }
            Message::Unlocked(seq, result) => self.unlocked(seq, result),
            Message::KeySaved(seq, result) => self.key_saved(seq, result),
            Message::DismissError => {
                self.local = true;
                self.error = None;
                Task::none()
            }
            // `App::handle` takes these before they get here.
            Message::ThemeMode(_)
            | Message::Shoot
            | Message::Shot(_)
            | Message::WindowOpened(_) => {
                return Task::none();
            }
        };
        let mut extra = Task::none();
        if std::mem::take(&mut self.tasks_changed) {
            // The Weekly Summary follows changes on the Week tab, once they
            // pause for `SUMMARY_WAIT_SECS`.
            if self.tab == Tab::Week {
                self.reports.start_summary_wait();
            }
            // A decision model is quick: recommendations follow every change.
            // Run before `refresh()` so `recommending` and cleared later-day
            // recommendations appear immediately in this frame.
            if self.decision_active() {
                extra = self.update_week_reports();
            }
        }
        let local = std::mem::take(&mut self.local);
        if !local || self.must_refresh {
            self.refresh();
        }
        Task::batch([task, extra])
    }

    fn top_bar(&mut self, message: top_bar::Message) -> Task<Message> {
        match message {
            top_bar::Message::Tab(tab) => self.show(tab, None),
            top_bar::Message::Prev => {
                self.selected = top_bar::step(self.tab, self.selected, -1);
                self.date_picker = None;
                self.update_week_reports()
            }
            top_bar::Message::Next => {
                self.selected = top_bar::step(self.tab, self.selected, 1);
                self.date_picker = None;
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
            top_bar::Message::Search => self.open_search(),
            top_bar::Message::Lock => {
                self.date_picker = None;
                if self.keys.any_key_state() == KeyState::Unlocked {
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

    /// Closes the unlock prompt, by any route, wiping what was typed.
    fn close_unlock(&mut self) {
        self.unlocking = false;
        self.unlock_seq += 1;
        self.unlock_view.reset();
    }

    // ---- view ----

    /// The page, then the layers over it: a dark layer, the task dialog or
    /// Settings, another dark layer, and the unlock prompt (which can open
    /// over Settings). Every layer is always there, an empty space when
    /// closed: a dark layer can then fade out after its dialog closes, and
    /// nothing under a closing layer moves in the widget tree (the page
    /// keeps its scroll position, an open panel its state).
    fn view(&self) -> Element<'_, Message> {
        let today = self.today();
        let (dimmed, panel) = self.overlay(today);
        let none = || Element::from(Space::new());
        let unlock = if self.unlocking {
            opaque(self.unlock_view.view().map(Message::Unlock))
        } else {
            none()
        };
        let unlock_dimmed = if self.unlocking { theme::BACKDROP } else { 0.0 };
        stack![
            self.page(today),
            motion::dim(dimmed),
            panel.unwrap_or_else(none),
            motion::dim(unlock_dimmed),
            unlock,
        ]
        .into()
    }

    /// The top bar, the open tab, and any notice or error under it.
    fn page(&self, today: Date) -> Element<'_, Message> {
        let bar = top_bar::view(top_bar::Bar {
            tab: self.tab,
            selected: self.selected,
            today,
            schedule: &self.settings.schedule,
            lock: match self.keys.any_key_state() {
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
        // Another tab fades in; another day, week or month slides in from
        // the side it lies on.
        let (group, position) = self.page_number();
        let content = motion::appear(content)
            .rise(motion::RISE)
            .fade_from(|theme: &Theme| theme.palette().background)
            .page(group, position);

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
        main.height(Fill).into()
    }

    /// Which tab is open, for `motion::Appear::page`: changing tabs plays the
    /// rise-and-fade, while period changes slide inside each tab's own view
    /// (`motion::slide`) so they are not double-animated.
    fn page_number(&self) -> (u32, i64) {
        match self.tab {
            Tab::Today => (0, 0),
            Tab::Week => (1, 0),
            Tab::Month => (2, 0),
        }
    }

    /// What is open over the page: how dark the page goes, and the task
    /// dialog or Settings in a layer that takes every click (one outside
    /// the panel closes it).
    fn overlay(&self, today: Date) -> (f32, Option<Element<'_, Message>>) {
        if self.shortcuts_open {
            let panel = shortcuts_help::view().map(Message::ShortcutsHelp);
            let layer = opaque(
                mouse_area(
                    container(opaque(motion::appear(panel).rise(motion::RISE)))
                        .padding([32, 24])
                        .center(Fill),
                )
                .on_press(Message::CloseShortcuts),
            );
            return (theme::DIALOG_BACKDROP, Some(layer));
        }
        if self.search_open {
            let panel = self.search_view.view().map(Message::Search);
            let layer = opaque(
                mouse_area(
                    container(opaque(motion::appear(panel).rise(motion::RISE)))
                        .padding([48, 24])
                        .center_x(Fill)
                        .height(Fill),
                )
                .on_press(Message::CloseSearch),
            );
            return (theme::DIALOG_BACKDROP, Some(layer));
        }
        if let Some(dialog) = &self.dialog {
            let panel = container(
                task_form::task_dialog(&dialog.form, dialog.day, dialog.task.is_some(), today)
                    .map(Message::Dialog),
            )
            .max_width(640)
            .style(theme::overlay_panel);
            let layer = opaque(
                mouse_area(
                    container(opaque(motion::appear(panel).rise(motion::RISE)))
                        .padding([32, 24])
                        .center(Fill),
                )
                .on_press(Message::Dialog(day::Message::Close)),
            );
            return (theme::DIALOG_BACKDROP, Some(layer));
        }
        if !self.settings_open {
            return (0.0, None);
        }
        let panel = container(self.settings_view.view().map(Message::Settings))
            .max_width(760)
            .height(Fill)
            .style(theme::overlay_panel);
        let layer = opaque(
            mouse_area(
                container(opaque(motion::appear(panel).rise(motion::RISE)))
                    .padding([32, 24])
                    .center_x(Fill)
                    .height(Fill),
            )
            .on_press(Message::CloseSettings),
        );
        (theme::BACKDROP, Some(layer))
    }
}

/// Takes the keyboard focus from whatever has it. The page keeps its
/// widget state while a dialog or Settings is open over it, so a field left
/// focused there would go on taking what is typed.
fn unfocus() -> Task<Message> {
    iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus())
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

#[cfg(test)]
mod tests {
    use super::*;

    use jiff::civil::date;

    #[test]
    fn the_selection_follows_a_new_day_only_from_the_old_today() {
        let (old, new) = (date(2026, 11, 17), date(2026, 11, 18));
        assert_eq!(follow_today(old, old, new), new);
        let elsewhere = date(2026, 11, 25);
        assert_eq!(follow_today(elsewhere, old, new), elsewhere);
    }
}
