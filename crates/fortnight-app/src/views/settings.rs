//! Settings overlay and the unlock prompt. Owner: agent M (round 2; was
//! agent I).
//!
//! Settings opens as an overlay panel over the current tab. It is one
//! scrollable column under a fixed header that holds the title, the single
//! Save button (it saves the whole form: endpoint and protocol, schedule,
//! capacity, notifications, summary style) and a close button. Closing with
//! unsaved changes is allowed: the header says "Unsaved changes", and the
//! edits stay in the form until the saved settings change.
//!
//! The API key controls act immediately and are not part of Save. "Load
//! models" asks `main.rs` for the endpoint's model list; the result is
//! offered in a pick list beside the free-text model field, and is hidden
//! once the protocol or base URL is edited away from the listed endpoint.
//!
//! Focus hours per day are typed as decimal hours (`5.5`) and stored as
//! whole minutes; an invalid value blocks Save like an invalid time does.
//!
//! Secrets (API key, passwords) live in the form as `String` only while
//! typed. They are moved into a `SecretString` when an action is emitted,
//! and every other secret field is wiped at the same time. `Message` prints
//! them as `***` in `Debug` output.
//!
//! Skipped for the prototype: summary length/tone presets (free-form
//! instructions only), a "working…" indicator while Argon2 runs, selectable
//! data-folder text (iced 0.14 has no selectable text widget), and a
//! confirmation before closing with unsaved changes.

use fortnight_core::domain::{LlmSettings, Protocol, Schedule, Settings};
use fortnight_core::vault::MIN_PASSWORD_LEN;
use iced::widget::{
    Space, button, checkbox, column, container, operation, pick_list, row, rule, scrollable, text,
    text_input,
};
use iced::{Background, Border, Center, Color, Element, Fill, Length, Theme};
use jiff::civil::{Time, Weekday};
use secrecy::SecretString;
use std::fmt;
use std::path::PathBuf;

use crate::theme::{self, Icon};

/// The state of the encrypted API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// No key file: the endpoint needs no key, or none is set yet.
    NoKey,
    Locked,
    Unlocked,
}

/// Typed text from a secret field. Its `Debug` output is `***`.
#[derive(Clone)]
pub struct Typed(pub String);

impl fmt::Debug for Typed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// The secret fields of the key forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretField {
    ApiKey,
    Password,
    Confirm,
    OldPassword,
    NewPassword,
    NewConfirm,
    UnlockPassword,
}

#[derive(Debug, Clone)]
pub enum Message {
    Protocol(Protocol),
    BaseUrl(String),
    Model(String),
    /// Ask the endpoint for its model list.
    LoadModels,
    /// A model picked from the listed ones.
    PickModel(String),
    FocusHours(String),
    Notifications(bool),
    Close,
    Save,
    Test,
    WeekStart(Weekday),
    SummaryDay(Weekday),
    SummaryTime(String),
    PlanDay(Weekday),
    PlanTime(String),
    /// Monday-based index (0 = Monday) and the new value.
    WorkingDay(usize, bool),
    SummaryInstructions(String),
    Secret(SecretField, Typed),
    /// Submit the set/replace key form.
    SaveKey,
    SubmitChangePassword,
    Unlock,
    Lock,
    /// Open the replace-key form (Unlocked).
    ShowReplace,
    /// Open the change-password form (Unlocked).
    ShowChangePassword,
    /// First click on Remove key: ask for confirmation.
    AskRemove,
    ConfirmRemove,
    /// Close whichever key form or confirmation is open.
    CancelKey,
}

/// What `main.rs` must do. FROZEN contract (round 2).
#[derive(Debug, Clone)]
pub enum Action {
    None,
    /// Close the overlay (the close button or Escape).
    Close,
    /// Fetch the endpoint's model list using these endpoint fields and the
    /// current key. The result arrives as `SettingsData::models`.
    ListModels(LlmSettings),
    /// Save these settings (endpoint, schedule, summary instructions).
    Save(Settings),
    /// Send a test request using these endpoint fields and the current key.
    Test(LlmSettings),
    /// Encrypt and store a new API key (replacing any existing one).
    SetKey {
        api_key: SecretString,
        password: SecretString,
    },
    ChangePassword {
        old: SecretString,
        new: SecretString,
    },
    RemoveKey,
    Lock,
    Unlock(SecretString),
}

/// Everything the view shows, supplied by `main.rs`.
#[derive(Debug, Clone)]
pub struct SettingsData {
    pub settings: Settings,
    pub data_dir: PathBuf,
    pub key_state: KeyState,
    /// Result of the last Test: `Ok(model reply)` or `Err(message)`.
    pub test_result: Option<Result<String, String>>,
    pub testing: bool,
    /// Result of the last key operation (set, change, remove, unlock),
    /// shown near the key controls. `Err` for failures such as a wrong
    /// password.
    pub key_message: Option<Result<String, String>>,
    /// Result of the last model-list fetch: `Ok(ids)` (may be empty when the
    /// endpoint can't say; fall back to free text) or `Err(message)`.
    pub models: Option<Result<Vec<String>, String>>,
    /// A model-list fetch is in flight.
    pub listing_models: bool,
}

/// Monday to Sunday.
const WEEK: [Weekday; 7] = [
    Weekday::Monday,
    Weekday::Tuesday,
    Weekday::Wednesday,
    Weekday::Thursday,
    Weekday::Friday,
    Weekday::Saturday,
    Weekday::Sunday,
];

/// A weekday with a `Display` impl, for `pick_list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Day(Weekday);

const DAYS: [Day; 7] = [
    Day(Weekday::Monday),
    Day(Weekday::Tuesday),
    Day(Weekday::Wednesday),
    Day(Weekday::Thursday),
    Day(Weekday::Friday),
    Day(Weekday::Saturday),
    Day(Weekday::Sunday),
];

impl fmt::Display for Day {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(day_name(self.0))
    }
}

fn day_name(day: Weekday) -> &'static str {
    match day {
        Weekday::Monday => "Monday",
        Weekday::Tuesday => "Tuesday",
        Weekday::Wednesday => "Wednesday",
        Weekday::Thursday => "Thursday",
        Weekday::Friday => "Friday",
        Weekday::Saturday => "Saturday",
        Weekday::Sunday => "Sunday",
    }
}

/// `HH:MM` (or `H:MM`), 24-hour.
pub fn parse_time(s: &str) -> Option<Time> {
    let (h, m) = s.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    if !h.chars().chain(m.chars()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    Time::new(h.parse().ok()?, m.parse().ok()?, 0, 0).ok()
}

pub fn format_time(t: Time) -> String {
    format!("{:02}:{:02}", t.hour(), t.minute())
}

/// Decimal hours per day (`6`, `5.5`, `7,25`) as whole minutes. More than 0
/// and at most 24 hours.
pub fn parse_hours(s: &str) -> Option<u32> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let hours: f64 = s.parse().ok()?;
    let minutes = (hours * 60.0).round();
    (1.0..=24.0 * 60.0)
        .contains(&minutes)
        .then_some(minutes as u32)
}

/// Minutes as decimal hours with at most two decimals: 360 → `6`,
/// 330 → `5.5`. [`parse_hours`] reads it back to the same minutes.
pub fn format_hours(minutes: u32) -> String {
    if minutes.is_multiple_of(60) {
        return (minutes / 60).to_string();
    }
    let s = format!("{:.2}", f64::from(minutes) / 60.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// What the model-list area shows.
#[derive(Debug, PartialEq, Eq)]
enum ModelsState<'a> {
    Hidden,
    Busy,
    /// The endpoint answered but listed no models.
    Empty,
    Error(&'a str),
    List(&'a [String]),
}

/// The editable copy of the non-secret settings.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Form {
    base_url: String,
    model: String,
    week_start: Weekday,
    summary_day: Weekday,
    summary_time: String,
    plan_day: Weekday,
    plan_time: String,
    /// Indexed Monday = 0.
    working_days: [bool; 7],
    summary_instructions: String,
    protocol: Protocol,
    /// Decimal hours as typed, e.g. `5.5`.
    focus_hours: String,
    notifications: bool,
}

impl Default for Form {
    fn default() -> Self {
        Form::from_settings(&Settings::default())
    }
}

impl Form {
    fn from_settings(s: &Settings) -> Self {
        let sched = &s.schedule;
        Form {
            base_url: s.llm.base_url.clone(),
            model: s.llm.model.clone(),
            week_start: sched.week_start,
            summary_day: sched.summary_day,
            summary_time: format_time(sched.summary_time),
            plan_day: sched.plan_day,
            plan_time: format_time(sched.plan_time),
            working_days: WEEK.map(|d| sched.working_days.contains(&d)),
            summary_instructions: s.summary_instructions.clone(),
            protocol: s.llm.protocol,
            focus_hours: format_hours(s.focus_minutes_per_day),
            notifications: s.notifications,
        }
    }

    /// The settings this form describes, or `None` if a time or the focus
    /// hours are invalid.
    fn to_settings(&self) -> Option<Settings> {
        Some(Settings {
            llm: self.llm(),
            schedule: Schedule {
                week_start: self.week_start,
                summary_day: self.summary_day,
                summary_time: parse_time(&self.summary_time)?,
                plan_day: self.plan_day,
                plan_time: parse_time(&self.plan_time)?,
                working_days: WEEK
                    .iter()
                    .zip(self.working_days)
                    .filter(|(_, on)| *on)
                    .map(|(d, _)| *d)
                    .collect(),
            },
            summary_instructions: self.summary_instructions.trim().to_string(),
            focus_minutes_per_day: parse_hours(&self.focus_hours)?,
            notifications: self.notifications,
        })
    }

    fn llm(&self) -> LlmSettings {
        LlmSettings {
            protocol: self.protocol,
            base_url: self.base_url.trim().to_string(),
            model: self.model.trim().to_string(),
        }
    }
}

/// Which key form or confirmation is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum KeyMode {
    #[default]
    Idle,
    Replace,
    ChangePassword,
    ConfirmRemove,
}

/// The secret fields. `Debug` never prints them.
#[derive(Default)]
struct Secrets {
    api_key: String,
    password: String,
    confirm: String,
    old_password: String,
    new_password: String,
    new_confirm: String,
    unlock_password: String,
}

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secrets(***)")
    }
}

impl Secrets {
    fn field(&mut self, field: SecretField) -> &mut String {
        match field {
            SecretField::ApiKey => &mut self.api_key,
            SecretField::Password => &mut self.password,
            SecretField::Confirm => &mut self.confirm,
            SecretField::OldPassword => &mut self.old_password,
            SecretField::NewPassword => &mut self.new_password,
            SecretField::NewConfirm => &mut self.new_confirm,
            SecretField::UnlockPassword => &mut self.unlock_password,
        }
    }

    fn wipe_all(&mut self) {
        for s in [
            &mut self.api_key,
            &mut self.password,
            &mut self.confirm,
            &mut self.old_password,
            &mut self.new_password,
            &mut self.new_confirm,
            &mut self.unlock_password,
        ] {
            wipe(s);
        }
    }
}

/// Clears a string, zeroising its buffer (via `SecretString`'s drop).
fn wipe(s: &mut String) {
    drop(SecretString::from(std::mem::take(s)));
}

/// Moves a string into a `SecretString`, leaving it empty.
fn take_secret(s: &mut String) -> SecretString {
    SecretString::from(std::mem::take(s))
}

/// Checks a new password and its confirmation.
fn check_new_password(password: &str, confirm: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(format!(
            "The password needs at least {MIN_PASSWORD_LEN} characters."
        ));
    }
    if password != confirm {
        return Err("The passwords don't match.".into());
    }
    Ok(())
}

#[derive(Debug)]
pub struct SettingsView {
    /// The settings last given to `load`; the form resets when they change.
    loaded: Option<Settings>,
    form: Form,
    data_dir: PathBuf,
    key_state: KeyState,
    test_result: Option<Result<String, String>>,
    testing: bool,
    key_message: Option<Result<String, String>>,
    secrets: Secrets,
    key_mode: KeyMode,
    /// Local validation error for the key forms.
    key_error: Option<String>,
    models: Option<Result<Vec<String>, String>>,
    listing_models: bool,
    /// The endpoint of the last "Load models" request, to hide a list that
    /// belongs to an endpoint the form no longer names.
    listed_for: Option<(Protocol, String)>,
}

impl Default for SettingsView {
    fn default() -> Self {
        SettingsView {
            loaded: None,
            form: Form::default(),
            data_dir: PathBuf::new(),
            key_state: KeyState::NoKey,
            test_result: None,
            testing: false,
            key_message: None,
            secrets: Secrets::default(),
            key_mode: KeyMode::Idle,
            key_error: None,
            models: None,
            listing_models: false,
            listed_for: None,
        }
    }
}

impl SettingsView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces what the view shows. `main.rs` calls this after EVERY
    /// message, so it must be cheap. It must not discard unsaved form edits
    /// (typed URL, model, key, passwords) unless `settings` itself changed
    /// from what was last loaded.
    pub fn load(&mut self, data: SettingsData) {
        if self.loaded.as_ref() != Some(&data.settings) {
            self.form = Form::from_settings(&data.settings);
            self.loaded = Some(data.settings);
        }
        if data.key_state != self.key_state {
            self.key_state = data.key_state;
            self.key_mode = KeyMode::Idle;
            self.key_error = None;
            self.secrets.wipe_all();
        }
        self.data_dir = data.data_dir;
        self.test_result = data.test_result;
        self.testing = data.testing;
        self.key_message = data.key_message;
        self.models = data.models;
        self.listing_models = data.listing_models;
    }

    /// The settings the form would save, if they differ from the loaded
    /// ones and every field is valid.
    fn savable(&self) -> Option<Settings> {
        let settings = self.form.to_settings()?;
        let loaded = self.loaded.clone().unwrap_or_default();
        (settings != loaded).then_some(settings)
    }

    fn is_dirty(&self) -> bool {
        let loaded = self.loaded.clone().unwrap_or_default();
        self.form != Form::from_settings(&loaded)
    }

    fn can_test(&self) -> bool {
        self.form.llm().is_configured() && !self.testing
    }

    fn can_list_models(&self) -> bool {
        !self.form.base_url.trim().is_empty() && !self.listing_models
    }

    fn models_state(&self) -> ModelsState<'_> {
        if self.listing_models {
            return ModelsState::Busy;
        }
        if let Some((protocol, base_url)) = &self.listed_for
            && (*protocol != self.form.protocol || *base_url != self.form.base_url.trim())
        {
            return ModelsState::Hidden;
        }
        match &self.models {
            None => ModelsState::Hidden,
            Some(Ok(ids)) if ids.is_empty() => ModelsState::Empty,
            Some(Ok(ids)) => ModelsState::List(ids),
            Some(Err(e)) => ModelsState::Error(e),
        }
    }

    pub fn update(&mut self, message: Message) -> Action {
        let form = &mut self.form;
        match message {
            Message::Protocol(p) => form.protocol = p,
            Message::BaseUrl(s) => form.base_url = s,
            Message::Model(s) | Message::PickModel(s) => form.model = s,
            Message::FocusHours(s) => form.focus_hours = s,
            Message::Notifications(on) => form.notifications = on,
            Message::LoadModels => {
                if self.can_list_models() {
                    let llm = self.form.llm();
                    self.listed_for = Some((llm.protocol, llm.base_url.clone()));
                    return Action::ListModels(llm);
                }
            }
            Message::Close => return Action::Close,
            Message::WeekStart(d) => form.week_start = d,
            Message::SummaryDay(d) => form.summary_day = d,
            Message::SummaryTime(s) => form.summary_time = s,
            Message::PlanDay(d) => form.plan_day = d,
            Message::PlanTime(s) => form.plan_time = s,
            Message::WorkingDay(i, on) => {
                if let Some(day) = form.working_days.get_mut(i) {
                    *day = on;
                }
            }
            Message::SummaryInstructions(s) => form.summary_instructions = s,
            Message::Save => {
                if let Some(settings) = self.savable() {
                    return Action::Save(settings);
                }
            }
            Message::Test => {
                if self.can_test() {
                    return Action::Test(self.form.llm());
                }
            }
            Message::Secret(field, Typed(value)) => {
                let slot = self.secrets.field(field);
                wipe(slot);
                *slot = value;
            }
            Message::SaveKey => return self.save_key(),
            Message::SubmitChangePassword => return self.change_password(),
            Message::Unlock => {
                if self.secrets.unlock_password.is_empty() {
                    return Action::None;
                }
                self.key_error = None;
                return Action::Unlock(take_secret(&mut self.secrets.unlock_password));
            }
            Message::Lock => {
                self.close_key_form();
                return Action::Lock;
            }
            Message::ShowReplace => self.open_key_form(KeyMode::Replace),
            Message::ShowChangePassword => self.open_key_form(KeyMode::ChangePassword),
            Message::AskRemove => self.open_key_form(KeyMode::ConfirmRemove),
            Message::ConfirmRemove => {
                if self.key_mode == KeyMode::ConfirmRemove {
                    self.close_key_form();
                    return Action::RemoveKey;
                }
            }
            Message::CancelKey => self.close_key_form(),
        }
        Action::None
    }

    fn open_key_form(&mut self, mode: KeyMode) {
        self.close_key_form();
        self.key_mode = mode;
    }

    fn close_key_form(&mut self) {
        self.key_mode = KeyMode::Idle;
        self.key_error = None;
        self.secrets.wipe_all();
    }

    fn save_key(&mut self) -> Action {
        let s = &self.secrets;
        let check = if s.api_key.trim().is_empty() {
            Err("Enter the API key.".to_string())
        } else {
            check_new_password(&s.password, &s.confirm)
        };
        if let Err(e) = check {
            self.key_error = Some(e);
            return Action::None;
        }
        let api_key = SecretString::from(self.secrets.api_key.trim().to_string());
        let password = take_secret(&mut self.secrets.password);
        self.close_key_form();
        Action::SetKey { api_key, password }
    }

    fn change_password(&mut self) -> Action {
        let s = &self.secrets;
        let check = if s.old_password.is_empty() {
            Err("Enter your current password.".to_string())
        } else {
            check_new_password(&s.new_password, &s.new_confirm)
        };
        if let Err(e) = check {
            self.key_error = Some(e);
            return Action::None;
        }
        let old = take_secret(&mut self.secrets.old_password);
        let new = take_secret(&mut self.secrets.new_password);
        self.close_key_form();
        Action::ChangePassword { old, new }
    }

    // ------------------------------------------------------------- view

    pub fn view(&self) -> Element<'_, Message> {
        let save = button(text("Save").size(theme::TEXT_BODY))
            .padding([8, 18])
            .style(theme::primary_button)
            .on_press_maybe(self.savable().map(|_| Message::Save));
        let close = button(theme::icon(Icon::Close))
            .padding(theme::ICON_PADDING)
            .style(theme::icon_button)
            .on_press(Message::Close);
        let status: Element<'_, Message> = if !self.is_dirty() {
            Space::new().into()
        } else if self.form.to_settings().is_none() {
            text("Fix the errors below to save")
                .size(theme::TEXT_SMALL)
                .style(theme::danger)
                .into()
        } else {
            text("Unsaved changes")
                .size(theme::TEXT_SMALL)
                .style(theme::muted)
                .into()
        };
        let header = container(
            row![
                theme::title("Settings"),
                Space::new().width(Fill),
                status,
                save,
                close
            ]
            .spacing(theme::SPACING)
            .align_y(Center),
        )
        .padding(iced::Padding {
            top: theme::SPACING * 1.5,
            bottom: theme::SPACING * 1.5,
            left: theme::SPACING * 2.0,
            right: theme::SPACING * 1.5,
        })
        .width(Fill);

        let content = column![
            self.endpoint_section(),
            self.key_section(),
            self.schedule_section(),
            self.capacity_section(),
            self.notifications_section(),
            self.summary_section(),
            self.data_section(),
        ]
        .spacing(theme::SPACING * 2.5)
        .padding(iced::Padding {
            top: theme::SPACING,
            bottom: theme::SPACING * 3.0,
            left: theme::SPACING * 2.0,
            right: theme::SPACING * 2.0,
        })
        .width(Fill);

        column![
            header,
            rule::horizontal(1).style(divider),
            scrollable(content)
                .direction(scrollable::Direction::Vertical(
                    scrollable::Scrollbar::new()
                        .width(6)
                        .scroller_width(6)
                        .margin(4)
                ))
                .height(Fill)
        ]
        .into()
    }

    fn endpoint_section(&self) -> Element<'_, Message> {
        let f = &self.form;
        let protocol = labelled(
            "Protocol",
            pick_list(Protocol::ALL, Some(f.protocol), Message::Protocol)
                .padding([8, 12])
                .text_size(theme::TEXT_BODY)
                .width(Fill)
                .style(picker),
        );
        let base_url = labelled(
            "Base URL",
            input(f.protocol.example_base_url(), &f.base_url).on_input(Message::BaseUrl),
        );
        let model_placeholder = match f.protocol {
            Protocol::OpenAi => "e.g. llama3.2 or gpt-4o",
            Protocol::Anthropic => "e.g. claude-sonnet-5-5",
        };
        let load_label = if self.listing_models {
            "Loading…"
        } else {
            "Load models"
        };
        let load = button(text(load_label).size(theme::TEXT_BODY))
            .padding([8, 14])
            .style(theme::secondary_button)
            .on_press_maybe(self.can_list_models().then_some(Message::LoadModels));
        let model = labelled(
            "Model",
            row![
                input(model_placeholder, &f.model).on_input(Message::Model),
                load
            ]
            .spacing(theme::SPACING / 2.0)
            .align_y(Center),
        );
        let models: Element<'_, Message> = match self.models_state() {
            ModelsState::Hidden => Space::new().into(),
            ModelsState::Busy => small_muted("Asking the endpoint for its models…"),
            ModelsState::Empty => {
                small_muted("The endpoint didn't list any models: type the model name above.")
            }
            ModelsState::Error(e) => text(format!("Couldn't load models: {}", shorten(e, 160)))
                .size(theme::TEXT_SMALL)
                .style(theme::danger)
                .into(),
            ModelsState::List(ids) => {
                let selected = ids.iter().find(|id| **id == f.model.trim()).cloned();
                row![
                    text(format!(
                        "{} model{} listed",
                        ids.len(),
                        if ids.len() == 1 { "" } else { "s" }
                    ))
                    .size(theme::TEXT_SMALL)
                    .style(theme::muted),
                    pick_list(ids, selected, Message::PickModel)
                        .placeholder("Choose a model")
                        .padding([6, 12])
                        .text_size(theme::TEXT_BODY)
                        .width(Fill)
                        .style(picker),
                ]
                .spacing(theme::SPACING)
                .align_y(Center)
                .into()
            }
        };
        let hint = small_muted(match f.protocol {
            Protocol::OpenAi => {
                "Any OpenAI-compatible endpoint works, e.g. Ollama at http://localhost:11434/v1."
            }
            Protocol::Anthropic => "Anthropic's Messages API, e.g. https://api.anthropic.com.",
        });

        let test_label = if self.testing {
            "Testing…"
        } else {
            "Test connection"
        };
        let test = button(text(test_label).size(theme::TEXT_BODY))
            .padding([8, 18])
            .style(theme::secondary_button)
            .on_press_maybe(self.can_test().then_some(Message::Test));
        let mut test_row = row![test].spacing(theme::SPACING).align_y(Center);
        if !self.testing {
            match &self.test_result {
                Some(Ok(reply)) => {
                    test_row = test_row.push(
                        text(format!("Connected. {}", shorten(reply, 80)))
                            .size(theme::TEXT_SMALL)
                            .style(success)
                            .width(Fill),
                    )
                }
                Some(Err(e)) => {
                    test_row = test_row.push(
                        text(shorten(e, 160))
                            .size(theme::TEXT_SMALL)
                            .style(theme::danger)
                            .width(Fill),
                    )
                }
                None if self.key_state == KeyState::Locked => {
                    test_row = test_row.push(small_muted("The key is locked: unlock it first."))
                }
                None => {}
            }
        }

        section(
            "LLM",
            column![
                row![
                    container(protocol).width(Length::FillPortion(2)),
                    container(base_url).width(Length::FillPortion(3))
                ]
                .spacing(theme::SPACING),
                hint,
                model,
                models,
                test_row
            ]
            .spacing(theme::SPACING),
        )
    }

    fn key_section(&self) -> Element<'_, Message> {
        let state = match self.key_state {
            KeyState::NoKey => "No key",
            KeyState::Locked => "Locked",
            KeyState::Unlocked => "Unlocked",
        };
        let chip = container(text(state).size(theme::TEXT_SMALL))
            .padding(theme::chip_padding())
            .style(theme::chip);
        let mut body = column![
            row![text("Status").size(theme::TEXT_BODY), chip]
                .spacing(theme::SPACING)
                .align_y(Center)
        ]
        .spacing(theme::SPACING);

        match (self.key_state, self.key_mode) {
            (_, KeyMode::ConfirmRemove) => {
                body = body.push(
                    row![
                        text("Really remove the key?").size(theme::TEXT_BODY),
                        button(text("Remove").size(theme::TEXT_BODY))
                            .padding([8, 14])
                            .style(theme::danger_button)
                            .on_press(Message::ConfirmRemove),
                        cancel_button(),
                    ]
                    .spacing(theme::SPACING)
                    .align_y(Center),
                );
            }
            (KeyState::NoKey, _) => {
                body = body.push(small_muted(
                    "Optional: a local endpoint such as Ollama needs no key. \
                     The key is stored encrypted with a password you choose.",
                ));
                body = body.push(self.set_key_form(false));
            }
            (KeyState::Locked, _) => {
                body = body.push(
                    row![
                        secret_input(
                            "Password",
                            &self.secrets.unlock_password,
                            SecretField::UnlockPassword
                        )
                        .on_submit(Message::Unlock)
                        .width(280),
                        primary("Unlock", Message::Unlock),
                        Space::new().width(Fill),
                        remove_button(),
                    ]
                    .spacing(theme::SPACING)
                    .align_y(Center),
                );
            }
            (KeyState::Unlocked, KeyMode::Replace) => {
                body = body.push(self.set_key_form(true));
            }
            (KeyState::Unlocked, KeyMode::ChangePassword) => {
                body = body.push(self.change_password_form());
            }
            (KeyState::Unlocked, KeyMode::Idle) => {
                body = body.push(
                    row![
                        secondary("Lock", Message::Lock),
                        secondary("Replace key", Message::ShowReplace),
                        secondary("Change password", Message::ShowChangePassword),
                        Space::new().width(Fill),
                        remove_button(),
                    ]
                    .spacing(theme::SPACING)
                    .align_y(Center),
                );
            }
        }

        if let Some(e) = &self.key_error {
            body = body.push(text(e).size(theme::TEXT_SMALL).style(theme::danger));
        } else {
            match &self.key_message {
                Some(Ok(m)) => body = body.push(text(m).size(theme::TEXT_SMALL).style(success)),
                Some(Err(e)) => {
                    body = body.push(text(e).size(theme::TEXT_SMALL).style(theme::danger))
                }
                None => {}
            }
        }
        if self.key_state != KeyState::NoKey {
            body = body.push(small_muted(
                "A forgotten password can't be recovered: remove the key and enter it again.",
            ));
        }

        section("API KEY", body)
    }

    fn set_key_form(&self, cancellable: bool) -> Element<'_, Message> {
        let s = &self.secrets;
        let mut buttons = row![primary("Save key", Message::SaveKey)].spacing(theme::SPACING);
        if cancellable {
            buttons = buttons.push(cancel_button());
        }
        column![
            labelled(
                "API key",
                secret_input("sk-…", &s.api_key, SecretField::ApiKey)
            ),
            row![
                labelled(
                    "Password",
                    secret_input(
                        &format!("At least {MIN_PASSWORD_LEN} characters"),
                        &s.password,
                        SecretField::Password
                    )
                ),
                labelled(
                    "Confirm password",
                    secret_input("Same again", &s.confirm, SecretField::Confirm)
                        .on_submit(Message::SaveKey)
                ),
            ]
            .spacing(theme::SPACING),
            buttons,
        ]
        .spacing(theme::SPACING)
        .into()
    }

    fn change_password_form(&self) -> Element<'_, Message> {
        let s = &self.secrets;
        column![
            labelled(
                "Current password",
                secret_input("", &s.old_password, SecretField::OldPassword)
            ),
            row![
                labelled(
                    "New password",
                    secret_input(
                        &format!("At least {MIN_PASSWORD_LEN} characters"),
                        &s.new_password,
                        SecretField::NewPassword
                    )
                ),
                labelled(
                    "Confirm new password",
                    secret_input("Same again", &s.new_confirm, SecretField::NewConfirm)
                        .on_submit(Message::SubmitChangePassword)
                ),
            ]
            .spacing(theme::SPACING),
            row![
                primary("Change password", Message::SubmitChangePassword),
                cancel_button()
            ]
            .spacing(theme::SPACING),
        ]
        .spacing(theme::SPACING)
        .into()
    }

    fn schedule_section(&self) -> Element<'_, Message> {
        let f = &self.form;
        let week_start = labelled(
            "Week starts on",
            day_picker(f.week_start, Message::WeekStart),
        );
        let summary = labelled_when(
            "Weekly summary",
            day_picker(f.summary_day, Message::SummaryDay),
            &f.summary_time,
            Message::SummaryTime,
        );
        let plan = labelled_when(
            "Week-ahead advice",
            day_picker(f.plan_day, Message::PlanDay),
            &f.plan_time,
            Message::PlanTime,
        );

        section(
            "SCHEDULE",
            column![week_start, row![summary, plan].spacing(theme::SPACING)]
                .spacing(theme::SPACING * 1.25),
        )
    }

    fn capacity_section(&self) -> Element<'_, Message> {
        let f = &self.form;
        let mut days = row![].spacing(theme::SPACING * 1.25);
        for (i, day) in WEEK.iter().enumerate() {
            days = days.push(
                checkbox(f.working_days[i])
                    .label(&day_name(*day)[..3])
                    .text_size(theme::TEXT_BODY)
                    .on_toggle(move |on| Message::WorkingDay(i, on)),
            );
        }
        let mut hours = column![
            text("Focus hours per day").size(theme::TEXT_SMALL),
            row![
                input("6", &f.focus_hours)
                    .on_input(Message::FocusHours)
                    .width(90),
                small_muted("Hours of real work a day; the Week tab compares estimates with it."),
            ]
            .spacing(theme::SPACING)
            .align_y(Center),
        ]
        .spacing(theme::SPACING / 3.0);
        if parse_hours(&f.focus_hours).is_none() {
            hours = hours.push(
                text("Use hours between 0 and 24, e.g. 6 or 5.5")
                    .size(theme::TEXT_SMALL)
                    .style(theme::danger),
            );
        }
        section(
            "CAPACITY",
            column![
                column![text("Working days").size(theme::TEXT_SMALL), days]
                    .spacing(theme::SPACING / 2.0),
                hours,
            ]
            .spacing(theme::SPACING * 1.25),
        )
    }

    fn notifications_section(&self) -> Element<'_, Message> {
        section(
            "NOTIFICATIONS",
            column![
                checkbox(self.form.notifications)
                    .label("Notify me when a report is ready")
                    .text_size(theme::TEXT_BODY)
                    .on_toggle(Message::Notifications),
                small_muted(
                    "Uses the system's notifications, which are best-effort: \
                     some systems hide or block them."
                ),
            ]
            .spacing(theme::SPACING / 2.0),
        )
    }

    fn summary_section(&self) -> Element<'_, Message> {
        section(
            "SUMMARY STYLE",
            column![
                input(
                    "e.g. Under 150 words, bullet points, plain tone",
                    &self.form.summary_instructions
                )
                .on_input(Message::SummaryInstructions),
                small_muted("Extra instructions added to the weekly summary prompt."),
            ]
            .spacing(theme::SPACING / 2.0),
        )
    }

    fn data_section(&self) -> Element<'_, Message> {
        section(
            "DATA FOLDER",
            text(self.data_dir.display().to_string())
                .size(theme::TEXT_SMALL)
                .style(theme::muted),
        )
    }
}

// ------------------------------------------------------------ view helpers

fn section<'a>(heading: &'a str, body: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    column![theme::section_heading(heading), body.into()]
        .spacing(theme::SPACING)
        .into()
}

fn labelled<'a>(label: &'a str, field: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    column![text(label).size(theme::TEXT_SMALL), field.into()]
        .spacing(theme::SPACING / 3.0)
        .width(Fill)
        .into()
}

/// A day picker and a time field, with an inline error for a bad time.
fn labelled_when<'a>(
    label: &'a str,
    day: Element<'a, Message>,
    time: &'a str,
    on_time: fn(String) -> Message,
) -> Element<'a, Message> {
    let time_input = input("HH:MM", time).on_input(on_time).width(90);
    let mut col = column![
        text(label).size(theme::TEXT_SMALL),
        row![day, time_input].spacing(theme::SPACING / 2.0)
    ]
    .spacing(theme::SPACING / 3.0)
    .width(Fill);
    if parse_time(time).is_none() {
        col = col.push(
            text("Use 24-hour HH:MM, e.g. 16:00")
                .size(theme::TEXT_SMALL)
                .style(theme::danger),
        );
    }
    col.into()
}

fn input<'a>(placeholder: &str, value: &str) -> text_input::TextInput<'a, Message> {
    text_input(placeholder, value)
        .padding([8, 12])
        .size(theme::TEXT_BODY)
        .style(theme::input)
}

fn secret_input<'a>(
    placeholder: &str,
    value: &str,
    field: SecretField,
) -> text_input::TextInput<'a, Message> {
    input(placeholder, value)
        .secure(true)
        .on_input(move |s| Message::Secret(field, Typed(s)))
}

fn day_picker<'a>(day: Weekday, on_select: fn(Weekday) -> Message) -> Element<'a, Message> {
    pick_list(DAYS, Some(Day(day)), move |d: Day| on_select(d.0))
        .padding([8, 12])
        .text_size(theme::TEXT_BODY)
        .width(150)
        .style(picker)
        .into()
}

fn primary<'a>(label: &'a str, message: Message) -> Element<'a, Message> {
    button(text(label).size(theme::TEXT_BODY))
        .padding([8, 18])
        .style(theme::primary_button)
        .on_press(message)
        .into()
}

fn secondary<'a>(label: &'a str, message: Message) -> Element<'a, Message> {
    button(text(label).size(theme::TEXT_BODY))
        .padding([8, 14])
        .style(theme::secondary_button)
        .on_press(message)
        .into()
}

fn cancel_button<'a>() -> Element<'a, Message> {
    button(text("Cancel").size(theme::TEXT_BODY))
        .padding([8, 14])
        .style(theme::text_button)
        .on_press(Message::CancelKey)
        .into()
}

fn remove_button<'a>() -> Element<'a, Message> {
    button(text("Remove key").size(theme::TEXT_BODY))
        .padding([8, 14])
        .style(theme::danger_button)
        .on_press(Message::AskRemove)
        .into()
}

fn small_muted<'a>(s: &'a str) -> Element<'a, Message> {
    text(s).size(theme::TEXT_SMALL).style(theme::muted).into()
}

/// First `max` characters of the first line, with an ellipsis if cut.
fn shorten(s: &str, max: usize) -> String {
    let line = s.trim().lines().next().unwrap_or("");
    if line.chars().count() > max || s.trim().lines().count() > 1 {
        let cut: String = line.chars().take(max).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

/// Success text: the theme's success colour.
fn success(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(theme.palette().success),
    }
}

/// The thin line under the fixed header.
fn divider(theme: &Theme) -> rule::Style {
    rule::Style {
        color: theme::muted_color(theme).scale_alpha(0.25),
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

/// A pick list styled like [`theme::input`].
fn picker(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let p = theme.palette();
    let mix = |amount: f32| {
        Color::from_rgb(
            p.background.r + (p.text.r - p.background.r) * amount,
            p.background.g + (p.text.g - p.background.g) * amount,
            p.background.b + (p.text.b - p.background.b) * amount,
        )
    };
    let ring = match status {
        pick_list::Status::Opened { .. } => theme::ACCENT,
        pick_list::Status::Hovered => mix(0.15),
        pick_list::Status::Active => Color::TRANSPARENT,
    };
    pick_list::Style {
        text_color: p.text,
        placeholder_color: theme::muted_color(theme),
        handle_color: theme::muted_color(theme),
        background: Background::Color(mix(0.075)),
        border: Border {
            radius: theme::RADIUS.into(),
            width: 1.0,
            color: ring,
        },
    }
}

// ------------------------------------------------------------ unlock prompt

const UNLOCK_INPUT_ID: &str = "unlock-password";

#[derive(Debug, Clone)]
pub enum UnlockMessage {
    Password(Typed),
    Submit,
    Skip,
}

/// What `main.rs` must do. Contract: do not change without the orchestrator.
#[derive(Debug, Clone)]
pub enum UnlockAction {
    None,
    Unlock(SecretString),
    /// Continue without unlocking; LLM features wait.
    Skip,
}

/// The password prompt shown at launch when a key file exists.
#[derive(Default)]
pub struct UnlockView {
    password: String,
    error: Option<String>,
}

impl fmt::Debug for UnlockView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnlockView")
            .field("password", &"***")
            .field("error", &self.error)
            .finish()
    }
}

impl UnlockView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Focuses the password field. Optional: `main.rs` may run this task
    /// when it shows the prompt.
    pub fn focus() -> iced::Task<UnlockMessage> {
        operation::focus(UNLOCK_INPUT_ID)
    }

    /// Shows an error such as "Wrong password" and clears the field.
    pub fn set_error(&mut self, message: String) {
        self.error = Some(message);
        wipe(&mut self.password);
    }

    pub fn update(&mut self, message: UnlockMessage) -> UnlockAction {
        match message {
            UnlockMessage::Password(Typed(s)) => {
                wipe(&mut self.password);
                self.password = s;
                UnlockAction::None
            }
            UnlockMessage::Submit => {
                if self.password.is_empty() {
                    return UnlockAction::None;
                }
                self.error = None;
                UnlockAction::Unlock(take_secret(&mut self.password))
            }
            UnlockMessage::Skip => {
                wipe(&mut self.password);
                UnlockAction::Skip
            }
        }
    }

    pub fn view(&self) -> Element<'_, UnlockMessage> {
        let field = text_input("Password", &self.password)
            .id(UNLOCK_INPUT_ID)
            .secure(true)
            .on_input(|s| UnlockMessage::Password(Typed(s)))
            .on_submit(UnlockMessage::Submit)
            .padding([10, 14])
            .size(theme::TEXT_BODY + 1.0)
            .style(theme::input);
        let unlock = button(container(text("Unlock").size(theme::TEXT_BODY)).center_x(Fill))
            .width(Fill)
            .padding([10, 18])
            .style(theme::primary_button)
            .on_press_maybe((!self.password.is_empty()).then_some(UnlockMessage::Submit));
        let skip = button(text("Skip — use without AI features").size(theme::TEXT_SMALL))
            .padding([4, 8])
            .style(theme::text_button)
            .on_press(UnlockMessage::Skip);

        let mut card = column![
            theme::title("Fortnight"),
            text("Enter your password to unlock the API key")
                .size(theme::TEXT_BODY)
                .style(theme::muted),
            Space::new().height(theme::SPACING / 2.0),
            field,
        ]
        .spacing(theme::SPACING);
        if let Some(e) = &self.error {
            card = card.push(text(e).size(theme::TEXT_SMALL).style(theme::danger));
        }
        card = card.push(unlock);
        card = card.push(container(skip).center_x(Fill));

        container(
            container(card)
                .padding(theme::SPACING * 2.5)
                .max_width(400)
                .style(theme::card),
        )
        .center(Fill)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fortnight_core::domain::DEFAULT_FOCUS_MINUTES;
    use secrecy::ExposeSecret;

    fn data(settings: Settings, key_state: KeyState) -> SettingsData {
        SettingsData {
            settings,
            data_dir: PathBuf::from("/tmp/fortnight-data"),
            key_state,
            test_result: None,
            testing: false,
            key_message: None,
            models: None,
            listing_models: false,
        }
    }

    fn loaded(key_state: KeyState) -> SettingsView {
        let mut v = SettingsView::new();
        v.load(data(Settings::default(), key_state));
        v
    }

    fn typed(v: &mut SettingsView, field: SecretField, s: &str) {
        v.update(Message::Secret(field, Typed(s.into())));
    }

    #[test]
    fn load_keeps_edits_when_settings_unchanged() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::BaseUrl("http://localhost:11434/v1".into()));
        typed(&mut v, SecretField::ApiKey, "sk-abc");
        v.load(data(Settings::default(), KeyState::NoKey));
        assert_eq!(v.form.base_url, "http://localhost:11434/v1");
        assert_eq!(v.secrets.api_key, "sk-abc");
    }

    #[test]
    fn load_resets_form_when_settings_change() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::BaseUrl("typed".into()));
        let mut s = Settings::default();
        s.llm.model = "gpt-4o".into();
        v.load(data(s, KeyState::NoKey));
        assert_eq!(v.form.base_url, "");
        assert_eq!(v.form.model, "gpt-4o");
    }

    #[test]
    fn save_emits_full_settings_with_parsed_times() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::BaseUrl(" https://api.openai.com/v1 ".into()));
        v.update(Message::Model("gpt-4o".into()));
        v.update(Message::SummaryDay(Weekday::Thursday));
        v.update(Message::SummaryTime("9:30".into()));
        v.update(Message::PlanTime("07:05".into()));
        v.update(Message::WorkingDay(4, false));
        v.update(Message::WorkingDay(5, true));
        v.update(Message::SummaryInstructions("Short.".into()));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.llm.base_url, "https://api.openai.com/v1");
        assert_eq!(s.llm.model, "gpt-4o");
        assert_eq!(s.schedule.summary_day, Weekday::Thursday);
        assert_eq!(s.schedule.summary_time, Time::constant(9, 30, 0, 0));
        assert_eq!(s.schedule.plan_time, Time::constant(7, 5, 0, 0));
        assert_eq!(s.schedule.week_start, Weekday::Monday);
        assert_eq!(
            s.schedule.working_days,
            vec![
                Weekday::Monday,
                Weekday::Tuesday,
                Weekday::Wednesday,
                Weekday::Thursday,
                Weekday::Saturday
            ]
        );
        assert_eq!(s.summary_instructions, "Short.");
    }

    #[test]
    fn save_does_nothing_when_unchanged() {
        let mut v = loaded(KeyState::NoKey);
        assert!(matches!(v.update(Message::Save), Action::None));
    }

    #[test]
    fn invalid_time_blocks_save() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::Model("m".into()));
        v.update(Message::PlanTime("25:00".into()));
        assert!(matches!(v.update(Message::Save), Action::None));
        v.update(Message::PlanTime("8am".into()));
        assert!(matches!(v.update(Message::Save), Action::None));
        v.update(Message::PlanTime("08:00".into()));
        assert!(matches!(v.update(Message::Save), Action::Save(_)));
    }

    #[test]
    fn parse_time_accepts_only_hh_mm() {
        assert_eq!(parse_time("16:00"), Some(Time::constant(16, 0, 0, 0)));
        assert_eq!(parse_time("8:05"), Some(Time::constant(8, 5, 0, 0)));
        for bad in [
            "", "16", "16:0", "24:00", "12:60", "1:2:3", "+1:00", "ab:cd",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }

    #[test]
    fn set_key_validation() {
        let cases = [
            ("", "longenough", "longenough"),
            ("sk-abc", "short", "short"),
            ("sk-abc", "longenough", "different1"),
        ];
        for (key, pw, confirm) in cases {
            let mut v = loaded(KeyState::NoKey);
            typed(&mut v, SecretField::ApiKey, key);
            typed(&mut v, SecretField::Password, pw);
            typed(&mut v, SecretField::Confirm, confirm);
            assert!(matches!(v.update(Message::SaveKey), Action::None));
            assert!(v.key_error.is_some(), "{key}/{pw}/{confirm}");
            // The typed values stay so the user can fix them.
            assert_eq!(v.secrets.password, pw);
        }
    }

    #[test]
    fn valid_set_key_emits_and_clears_fields() {
        let mut v = loaded(KeyState::NoKey);
        typed(&mut v, SecretField::ApiKey, " sk-abc ");
        typed(&mut v, SecretField::Password, "correct horse");
        typed(&mut v, SecretField::Confirm, "correct horse");
        let Action::SetKey { api_key, password } = v.update(Message::SaveKey) else {
            panic!("expected SetKey");
        };
        assert_eq!(api_key.expose_secret(), "sk-abc");
        assert_eq!(password.expose_secret(), "correct horse");
        assert!(v.secrets.api_key.is_empty());
        assert!(v.secrets.password.is_empty());
        assert!(v.secrets.confirm.is_empty());
        assert!(v.key_error.is_none());
    }

    #[test]
    fn change_password_validates_and_emits() {
        let mut v = loaded(KeyState::Unlocked);
        v.update(Message::ShowChangePassword);
        typed(&mut v, SecretField::OldPassword, "old password");
        typed(&mut v, SecretField::NewPassword, "new password");
        typed(&mut v, SecretField::NewConfirm, "new passw0rd");
        assert!(matches!(
            v.update(Message::SubmitChangePassword),
            Action::None
        ));
        assert!(v.key_error.is_some());
        typed(&mut v, SecretField::NewConfirm, "new password");
        let Action::ChangePassword { old, new } = v.update(Message::SubmitChangePassword) else {
            panic!("expected ChangePassword");
        };
        assert_eq!(old.expose_secret(), "old password");
        assert_eq!(new.expose_secret(), "new password");
        assert!(v.secrets.new_confirm.is_empty());
        assert_eq!(v.key_mode, KeyMode::Idle);
    }

    #[test]
    fn test_emits_current_fields() {
        let mut v = loaded(KeyState::NoKey);
        assert!(matches!(v.update(Message::Test), Action::None));
        v.update(Message::BaseUrl("http://localhost:11434/v1".into()));
        v.update(Message::Model("llama3.2".into()));
        let Action::Test(llm) = v.update(Message::Test) else {
            panic!("expected Test");
        };
        assert_eq!(llm.base_url, "http://localhost:11434/v1");
        assert_eq!(llm.model, "llama3.2");
        // Not while a test is running.
        let mut d = data(Settings::default(), KeyState::NoKey);
        d.testing = true;
        v.load(d);
        assert!(matches!(v.update(Message::Test), Action::None));
    }

    #[test]
    fn remove_key_needs_confirmation() {
        let mut v = loaded(KeyState::Unlocked);
        assert!(matches!(v.update(Message::ConfirmRemove), Action::None));
        v.update(Message::AskRemove);
        assert_eq!(v.key_mode, KeyMode::ConfirmRemove);
        v.update(Message::CancelKey);
        assert_eq!(v.key_mode, KeyMode::Idle);
        v.update(Message::AskRemove);
        assert!(matches!(
            v.update(Message::ConfirmRemove),
            Action::RemoveKey
        ));
        assert_eq!(v.key_mode, KeyMode::Idle);
    }

    #[test]
    fn settings_unlock_and_lock() {
        let mut v = loaded(KeyState::Locked);
        assert!(matches!(v.update(Message::Unlock), Action::None));
        typed(&mut v, SecretField::UnlockPassword, "hunter22");
        let Action::Unlock(pw) = v.update(Message::Unlock) else {
            panic!("expected Unlock");
        };
        assert_eq!(pw.expose_secret(), "hunter22");
        assert!(v.secrets.unlock_password.is_empty());
        v.load(data(Settings::default(), KeyState::Unlocked));
        assert!(matches!(v.update(Message::Lock), Action::Lock));
    }

    #[test]
    fn key_state_change_closes_forms_and_wipes_secrets() {
        let mut v = loaded(KeyState::Unlocked);
        v.update(Message::ShowReplace);
        typed(&mut v, SecretField::ApiKey, "sk-abc");
        v.load(data(Settings::default(), KeyState::NoKey));
        assert_eq!(v.key_mode, KeyMode::Idle);
        assert!(v.secrets.api_key.is_empty());
    }

    #[test]
    fn debug_never_prints_secrets() {
        let mut v = loaded(KeyState::NoKey);
        let m = Message::Secret(SecretField::ApiKey, Typed("sk-secret".into()));
        assert!(!format!("{m:?}").contains("sk-secret"));
        v.update(m);
        assert!(!format!("{v:?}").contains("sk-secret"));
        let um = UnlockMessage::Password(Typed("pw-secret".into()));
        assert!(!format!("{um:?}").contains("pw-secret"));
        let mut u = UnlockView::new();
        u.update(um);
        assert!(!format!("{u:?}").contains("pw-secret"));
    }

    #[test]
    fn unlock_prompt() {
        let mut u = UnlockView::new();
        assert!(matches!(
            u.update(UnlockMessage::Submit),
            UnlockAction::None
        ));
        u.update(UnlockMessage::Password(Typed("hunter22".into())));
        let UnlockAction::Unlock(pw) = u.update(UnlockMessage::Submit) else {
            panic!("expected Unlock");
        };
        assert_eq!(pw.expose_secret(), "hunter22");
        assert!(u.password.is_empty());
        assert!(matches!(u.update(UnlockMessage::Skip), UnlockAction::Skip));
    }

    #[test]
    fn protocol_and_focus_hours_round_trip_through_save() {
        let mut v = loaded(KeyState::NoKey);
        assert_eq!(v.form.focus_hours, "6");
        v.update(Message::Protocol(Protocol::Anthropic));
        v.update(Message::BaseUrl("https://api.anthropic.com".into()));
        v.update(Message::Model("claude-x".into()));
        v.update(Message::FocusHours(" 5.5 ".into()));
        v.update(Message::Notifications(true));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.llm.protocol, Protocol::Anthropic);
        assert_eq!(s.focus_minutes_per_day, 330);
        assert!(s.notifications);
        // Saved settings come back from main.rs: the form matches them and
        // there is nothing left to save.
        v.load(data(s.clone(), KeyState::NoKey));
        assert_eq!(v.form.protocol, Protocol::Anthropic);
        assert_eq!(v.form.focus_hours, "5.5");
        assert!(v.form.notifications);
        assert!(!v.is_dirty());
        assert!(matches!(v.update(Message::Save), Action::None));
    }

    #[test]
    fn round1_settings_load_with_defaults_and_save_unchanged() {
        // Round 1 settings JSON has no protocol, focus minutes or
        // notifications; core's serde defaults fill them in (tested in
        // `domain`), which gives this value.
        let mut round1 = Settings::default();
        round1.llm.base_url = "http://localhost:11434/v1".into();
        round1.llm.model = "llama3.2".into();
        round1.summary_instructions = "Short.".into();
        let mut v = SettingsView::new();
        v.load(data(round1.clone(), KeyState::NoKey));
        assert_eq!(v.form.protocol, Protocol::OpenAi);
        assert_eq!(v.form.focus_hours, "6");
        assert!(!v.is_dirty());
        v.update(Message::SummaryInstructions("Shorter.".into()));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.llm, round1.llm);
        assert_eq!(s.schedule, round1.schedule);
        assert_eq!(s.focus_minutes_per_day, DEFAULT_FOCUS_MINUTES);
        assert!(!s.notifications);
        assert_eq!(s.summary_instructions, "Shorter.");
    }

    #[test]
    fn invalid_focus_hours_block_save() {
        let mut v = loaded(KeyState::NoKey);
        for bad in ["", "0", "abc", "25", "-3", "5h", "1.2.3"] {
            v.update(Message::FocusHours(bad.into()));
            assert!(matches!(v.update(Message::Save), Action::None), "{bad}");
            assert!(v.is_dirty());
        }
        v.update(Message::FocusHours("7,5".into()));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.focus_minutes_per_day, 450);
    }

    #[test]
    fn hours_format_and_parse_round_trip() {
        assert_eq!(format_hours(360), "6");
        assert_eq!(format_hours(330), "5.5");
        assert_eq!(format_hours(345), "5.75");
        for minutes in 1..=24 * 60 {
            assert_eq!(
                parse_hours(&format_hours(minutes)),
                Some(minutes),
                "{minutes}"
            );
        }
        assert_eq!(parse_hours("24"), Some(1440));
        assert_eq!(parse_hours("24.5"), None);
    }

    #[test]
    fn list_models_carries_current_fields() {
        let mut v = loaded(KeyState::NoKey);
        // Needs a base URL.
        assert!(matches!(v.update(Message::LoadModels), Action::None));
        v.update(Message::Protocol(Protocol::Anthropic));
        v.update(Message::BaseUrl(" https://api.anthropic.com ".into()));
        v.update(Message::Model("typed".into()));
        let Action::ListModels(llm) = v.update(Message::LoadModels) else {
            panic!("expected ListModels");
        };
        assert_eq!(llm.protocol, Protocol::Anthropic);
        assert_eq!(llm.base_url, "https://api.anthropic.com");
        assert_eq!(llm.model, "typed");
        // Not while a fetch is in flight.
        let mut d = data(Settings::default(), KeyState::NoKey);
        d.listing_models = true;
        v.load(d);
        assert_eq!(v.models_state(), ModelsState::Busy);
        assert!(matches!(v.update(Message::LoadModels), Action::None));
    }

    #[test]
    fn models_result_handling() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::BaseUrl("http://localhost:11434/v1".into()));
        assert_eq!(v.models_state(), ModelsState::Hidden);
        v.update(Message::LoadModels);

        let with = |models| {
            let mut d = data(Settings::default(), KeyState::NoKey);
            d.models = Some(models);
            d
        };
        v.load(with(Ok(vec![])));
        assert_eq!(v.models_state(), ModelsState::Empty);

        v.load(with(Err("404 Not Found".into())));
        assert_eq!(v.models_state(), ModelsState::Error("404 Not Found"));

        let ids = vec!["llama3.2".to_string(), "qwen3".to_string()];
        v.load(with(Ok(ids.clone())));
        assert_eq!(v.models_state(), ModelsState::List(&ids));
        v.update(Message::PickModel("qwen3".into()));
        assert_eq!(v.form.model, "qwen3");
        // Free text still works after picking.
        v.update(Message::Model("qwen3:8b".into()));
        assert_eq!(v.form.model, "qwen3:8b");
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.llm.model, "qwen3:8b");

        // A list for another endpoint is hidden.
        v.update(Message::Protocol(Protocol::Anthropic));
        assert_eq!(v.models_state(), ModelsState::Hidden);
        v.update(Message::Protocol(Protocol::OpenAi));
        assert_eq!(v.models_state(), ModelsState::List(&ids));
    }

    #[test]
    fn close_emits_close_and_keeps_edits() {
        let mut v = loaded(KeyState::NoKey);
        assert!(matches!(v.update(Message::Close), Action::Close));
        v.update(Message::Model("unsaved".into()));
        assert!(matches!(v.update(Message::Close), Action::Close));
        v.load(data(Settings::default(), KeyState::NoKey));
        assert_eq!(v.form.model, "unsaved");
    }

    #[test]
    fn unlock_set_error_clears_field() {
        let mut u = UnlockView::new();
        u.update(UnlockMessage::Password(Typed("wrong".into())));
        u.set_error("wrong password".into());
        assert!(u.password.is_empty());
        assert_eq!(u.error.as_deref(), Some("wrong password"));
        u.update(UnlockMessage::Password(Typed("right".into())));
        assert!(matches!(
            u.update(UnlockMessage::Submit),
            UnlockAction::Unlock(_)
        ));
        assert!(u.error.is_none());
    }
}
