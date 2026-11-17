//! Fortnight: the application shell. Owns all state, carries out the actions
//! the views return, talks to the store, the key vault and the LLM, and runs
//! the minute tick that generates due reports.

use fortnight_app::theme;
use fortnight_app::views::review::{self, ReviewData, ReviewView};
use fortnight_app::views::settings::{
    self, KeyState, SettingsData, SettingsView, UnlockAction, UnlockMessage, UnlockView,
};
use fortnight_app::views::tasks::{self, TasksView};
use fortnight_app::views::week::{self, WeekData, WeekView};
use fortnight_core::datadir;
use fortnight_core::domain::{
    LlmSettings, NewReport, ReportKey, ReportKind, Settings, Task as CoreTask, WeekRef,
};
use fortnight_core::llm::{ChatRequest, LlmClient, LlmConfig};
use fortnight_core::openai::OpenAiClient;
use fortnight_core::report;
use fortnight_core::sqlite::SqliteStore;
use fortnight_core::store::Store;
use fortnight_core::vault::{self, VaultError};
use fortnight_core::week::{due_reports, week_containing, week_label};
use iced::keyboard::{self, Key};
use iced::widget::{button, center, column, container, row, text};
use iced::{Element, Fill, Subscription, Task, Theme};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ZonedDateTime};
use secrecy::SecretString;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// A failed automatic generation is not retried for this long.
const RETRY_AFTER_MINUTES: i64 = 10;

pub fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("Fortnight")
        .theme(App::theme)
        .subscription(App::subscription)
        .window_size((1100.0, 760.0))
        .run()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Tasks,
    Week,
    Review,
    Settings,
}

#[derive(Debug, Clone)]
enum Message {
    Nav(Page),
    Tasks(tasks::Message),
    Week(week::Message),
    Review(review::Message),
    Settings(settings::Message),
    Unlock(UnlockMessage),
    Tick,
    ThemeMode(iced::theme::Mode),
    Generated(ReportKey, NewReport),
    Tested(Result<String, String>),
    Unlocked(Result<SecretString, VaultError>),
    KeySaved(Result<SecretString, VaultError>),
    PasswordChanged(Result<(), VaultError>),
    DismissError,
    Shoot,
    Shot(iced::window::Screenshot),
}

/// Everything that needs the open database.
struct Ready {
    store: SqliteStore,
    data_dir: PathBuf,
    settings: Settings,
    tasks: Vec<CoreTask>,
    tz: TimeZone,

    page: Page,
    tasks_view: TasksView,
    week_view: WeekView,
    review_view: ReviewView,
    settings_view: SettingsView,
    unlock_view: UnlockView,
    /// The unlock prompt is showing (launch with a key file, until unlocked
    /// or skipped).
    unlocking: bool,

    key_exists: bool,
    api_key: Option<SecretString>,
    review_week: WeekRef,

    generating: HashSet<ReportKey>,
    last_attempt: HashMap<ReportKey, Timestamp>,
    testing: bool,
    test_result: Option<Result<String, String>>,
    key_message: Option<Result<String, String>>,
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
                    TasksView::focus_quick_add().map(Message::Tasks),
                    start,
                    shoot_after_delay(),
                ]);
                (App::Ready(ready, None), tasks)
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
                    Key::Character("1") => Some(Message::Nav(Page::Tasks)),
                    Key::Character("2") => Some(Message::Nav(Page::Week)),
                    Key::Character("3") => Some(Message::Nav(Page::Review)),
                    Key::Character(",") => Some(Message::Nav(Page::Settings)),
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
        let tz = TimeZone::system();
        let review_week =
            latest_summary_week(&Timestamp::now().to_zoned_datetime(tz.clone()), &settings);
        let key_exists = vault::key_path(&dir).exists();
        let page = match std::env::var("FORTNIGHT_PAGE").as_deref() {
            Ok("week") => Page::Week,
            Ok("review") => Page::Review,
            Ok("settings") => Page::Settings,
            _ => Page::Tasks,
        };
        let unlocking = key_exists && std::env::var_os("FORTNIGHT_SKIP_UNLOCK").is_none();
        Ok(Ready {
            store,
            data_dir: dir,
            settings,
            tasks,
            tz,
            page,
            tasks_view: TasksView::new(),
            week_view: WeekView::new(),
            review_view: ReviewView::new(),
            settings_view: SettingsView::new(),
            unlock_view: UnlockView::new(),
            unlocking,
            key_exists,
            api_key: None,
            review_week,
            generating: HashSet::new(),
            last_attempt: HashMap::new(),
            testing: false,
            test_result: None,
            key_message: None,
            error: None,
        })
    }

    fn now(&self) -> ZonedDateTime {
        Timestamp::now().to_zoned_datetime(self.tz.clone())
    }

    fn today(&self) -> Date {
        self.now().date()
    }

    fn current_week(&self) -> WeekRef {
        week_containing(self.today(), self.settings.schedule.week_start)
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
        let client = OpenAiClient::new(config).map_err(|e| e.to_string())?;
        Ok((Arc::new(client), host))
    }

    // ---- data flow ----

    fn reload_tasks(&mut self) {
        match self.store.list_tasks() {
            Ok(tasks) => self.tasks = tasks,
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Pushes fresh data into every view.
    fn refresh(&mut self) {
        let today = self.today();
        let current = self.current_week();

        let summary_facts = report::summary_facts(&self.tasks, self.review_week, today, &self.tz);
        let summary = self.latest(ReportKind::Summary, self.review_week);
        self.review_view.load(ReviewData {
            week: self.review_week,
            week_label: week_label(self.review_week),
            facts_md: report::summary_facts_markdown(&summary_facts),
            report: summary,
            generating: self.generating.contains(&ReportKey {
                kind: ReportKind::Summary,
                week_start: self.review_week.start,
            }),
            blocked: self.blocked(),
        });

        let plan_facts = report::plan_facts(&self.tasks, current, today, &self.settings);
        let advice = self.latest(ReportKind::Plan, current);
        self.week_view.load(WeekData {
            week: current,
            week_label: week_label(current),
            today,
            tasks: self.tasks.clone(),
            facts_md: report::plan_facts_markdown(&plan_facts),
            advice,
            generating: self.generating.contains(&ReportKey {
                kind: ReportKind::Plan,
                week_start: current.start,
            }),
            blocked: self.blocked(),
        });

        self.settings_view.load(SettingsData {
            settings: self.settings.clone(),
            data_dir: self.data_dir.clone(),
            key_state: self.key_state(),
            test_result: self.test_result.clone(),
            testing: self.testing,
            key_message: self.key_message.clone(),
        });
    }

    fn latest(
        &mut self,
        kind: ReportKind,
        week: WeekRef,
    ) -> Option<fortnight_core::domain::Report> {
        match self.store.latest_report(kind, week.start) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(e.to_string());
                None
            }
        }
    }

    fn key_state(&self) -> KeyState {
        match (self.key_exists, self.api_key.is_some()) {
            (false, _) => KeyState::NoKey,
            (true, false) => KeyState::Locked,
            (true, true) => KeyState::Unlocked,
        }
    }

    /// Starts generating a report unless one is already in flight.
    fn generate(&mut self, kind: ReportKind, week: WeekRef) -> Task<Message> {
        let key = ReportKey {
            kind,
            week_start: week.start,
        };
        if self.generating.contains(&key) {
            return Task::none();
        }
        let (client, host) = match self.client_for(&self.settings.llm.clone()) {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(e);
                return Task::none();
            }
        };
        let prepared = match kind {
            ReportKind::Summary => {
                report::prepare_summary(&self.tasks, week, self.today(), &self.tz, &self.settings)
            }
            ReportKind::Plan => {
                report::prepare_plan(&self.tasks, week, self.today(), &self.settings)
            }
        };
        self.generating.insert(key);
        self.last_attempt.insert(key, Timestamp::now());
        Task::perform(report::generate(client, prepared, host), move |r| {
            Message::Generated(key, r)
        })
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
            Message::Nav(page) => {
                self.page = page;
                if page == Page::Tasks {
                    TasksView::focus_quick_add().map(Message::Tasks)
                } else {
                    Task::none()
                }
            }
            Message::Tasks(m) => {
                let today = self.today();
                let action = self.tasks_view.update(m, &self.tasks, today);
                self.tasks_action(action);
                Task::none()
            }
            Message::Week(m) => match self.week_view.update(m) {
                week::Action::None => Task::none(),
                week::Action::Generate => {
                    let w = self.current_week();
                    self.generate(ReportKind::Plan, w)
                }
                week::Action::SetDone(id, done) => {
                    self.tasks_action(tasks::Action::SetDone(id, done));
                    Task::none()
                }
            },
            Message::Review(m) => match self.review_view.update(m) {
                review::Action::None => Task::none(),
                review::Action::ShowWeek(w) => {
                    self.review_week = w;
                    Task::none()
                }
                review::Action::Generate(w) => self.generate(ReportKind::Summary, w),
                review::Action::SaveEdit(id, text) => {
                    if let Err(e) = self.store.set_report_edit(id, Some(&text)) {
                        self.error = Some(e.to_string());
                    }
                    Task::none()
                }
                review::Action::Copy(text) => iced::clipboard::write(text),
            },
            Message::Settings(m) => {
                let action = self.settings_view.update(m);
                self.settings_action(action)
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
            Message::Generated(key, new) => {
                self.generating.remove(&key);
                if let Err(e) = self.store.insert_report(new) {
                    self.error = Some(e.to_string());
                }
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

    fn tasks_action(&mut self, action: tasks::Action) {
        let result = match action {
            tasks::Action::None => return,
            tasks::Action::Add(new) => self.store.add_task(new).map(|_| ()),
            tasks::Action::SetDone(id, done) => self.store.set_done(id, done),
            tasks::Action::Update(task) => self.store.update_task(&task),
            tasks::Action::Delete(id) => self.store.delete_task(id),
        };
        if let Err(e) = result {
            self.error = Some(e.to_string());
        }
        self.reload_tasks();
    }

    fn settings_action(&mut self, action: settings::Action) -> Task<Message> {
        let key_path = vault::key_path(&self.data_dir);
        match action {
            settings::Action::None => Task::none(),
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
                self.api_key = None; // dropping the SecretString zeroizes it
                self.key_message = Some(Ok("Locked.".into()));
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

        let nav = |label: &'static str, page: Page| {
            let style = if self.page == page {
                nav_active
            } else {
                theme::row_button
            };
            button(text(label))
                .width(Fill)
                .padding([8, 12])
                .style(style)
                .on_press(Message::Nav(page))
        };
        let sidebar = container(
            column![
                text("Fortnight")
                    .size(theme::TEXT_HEADING)
                    .style(theme::accent),
                column![
                    nav("Tasks", Page::Tasks),
                    nav("This week", Page::Week),
                    nav("Review", Page::Review),
                ]
                .spacing(4),
                iced::widget::space::vertical(),
                nav("Settings", Page::Settings),
            ]
            .spacing(theme::SPACING * 2.0)
            .padding(theme::SPACING),
        )
        .width(180)
        .height(Fill)
        .style(sidebar);

        let content: Element<'_, Message> = match self.page {
            Page::Tasks => {
                let today = self.today();
                self.tasks_view.view(&self.tasks, today).map(Message::Tasks)
            }
            Page::Week => self.week_view.view().map(Message::Week),
            Page::Review => self.review_view.view().map(Message::Review),
            Page::Settings => self.settings_view.view().map(Message::Settings),
        };

        let mut main = column![container(content).width(Fill).height(Fill)];
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

        row![sidebar, main].height(Fill).into()
    }
}

/// The sidebar: a faint neutral fill, no border.
fn sidebar(theme: &Theme) -> container::Style {
    let p = theme.palette();
    let mix = |a: f32, b: f32| a + (b - a) * 0.035;
    container::Style::default().background(iced::Color::from_rgb(
        mix(p.background.r, p.text.r),
        mix(p.background.g, p.text.g),
        mix(p.background.b, p.text.b),
    ))
}

/// The current page in the sidebar: accent text on a soft fill.
fn nav_active(theme: &Theme, status: button::Status) -> button::Style {
    let hovered = theme::row_button(theme, button::Status::Hovered);
    button::Style {
        text_color: theme::ACCENT,
        ..theme::row_button(theme, status).with_background(
            hovered
                .background
                .unwrap_or(iced::Background::Color(iced::Color::TRANSPARENT)),
        )
    }
}

/// The week the Review page opens on: the most recent week whose summary
/// is due (last week until Friday 16:00 by default, then this week).
fn latest_summary_week(now: &ZonedDateTime, settings: &Settings) -> WeekRef {
    due_reports(now, &settings.schedule, &[])
        .into_iter()
        .find(|job| job.kind == ReportKind::Summary)
        .map(|job| job.week)
        .unwrap_or_else(|| week_containing(now.date(), settings.schedule.week_start))
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
