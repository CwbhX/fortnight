//! Settings overlay and the unlock prompt.
//!
//! Settings opens as an overlay panel over the current tab. It is one
//! scrollable column under a fixed header that holds the title, the single
//! Save button (it saves the whole form: endpoint and protocol, schedule,
//! capacity and the two prompts) and a close button. Closing with unsaved
//! changes is allowed: the header says "Unsaved changes", and the edits stay
//! in the form until the saved settings change.
//!
//! The API key controls act immediately and are not part of Save. There is
//! one API key field; Save key (or Enter) asks for a password in a small
//! pop-up, which encrypts the key. Unlocking is done from the lock icon in
//! the top bar (the Unlock button here opens the same prompt).
//!
//! The prompts: the system prompts for the Work summary and Recommended
//! tasks, shown in full (the built-in one until changed) with Reset. A
//! prompt equal to the built-in one is saved as empty, so it follows future
//! built-in changes. Round 2's "summary instructions" are shown folded into
//! the summary prompt and are folded in for good once that prompt is saved.
//!
//! "Use local Ollama" fills in Ollama's OpenAI-compatible endpoint and
//! loads its model list; a local endpoint needs no API key. Reasoning
//! models (DeepSeek-R1, Qwen3, gpt-oss) work like any other: the client
//! drops their thinking.
//!
//! Secrets (API key, password) live in the form as `String` only while
//! typed. They are moved into a `SecretString` when an action is emitted,
//! and wiped otherwise. `Message` prints them as `***` in `Debug` output.
//!
//! Skipped for the prototype: a "working…" indicator while Argon2 runs,
//! selectable data-folder text (iced 0.14 has no selectable text widget),
//! and a confirmation before closing with unsaved changes. Notifications are
//! parked: the setting is kept but not shown.

use fortnight_core::domain::{AppIcon, LlmSettings, Protocol, Schedule, Settings};
use fortnight_core::report::{DEFAULT_PLAN_PROMPT, DEFAULT_SUMMARY_PROMPT};
use fortnight_core::secure::is_local_endpoint;
use fortnight_core::vault::MIN_PASSWORD_LEN;
use iced::widget::{
    Space, button, checkbox, column, container, opaque, operation, pick_list, row, rule,
    scrollable, stack, text, text_editor, text_input,
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
    /// The password pop-up.
    Password,
}

/// The two editable prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Summary,
    Plan,
}

#[derive(Debug, Clone)]
pub enum Message {
    Protocol(Which, Protocol),
    BaseUrl(Which, String),
    Model(Which, String),
    /// Ask the endpoint for its model list.
    LoadModels(Which),
    /// A model picked from the listed ones.
    PickModel(Which, String),
    /// Fill in a local Ollama server's endpoint and load its models.
    UseOllama(Which),
    /// Empty the decision model fields (recommendations use the main LLM).
    ClearDecision,
    FocusHours(String),
    Close,
    Save,
    Test(Which),
    WeekStart(Weekday),
    SummaryDay(Weekday),
    SummaryTime(String),
    PlanDay(Weekday),
    PlanTime(String),
    /// Monday-based index (0 = Monday) and the new value.
    WorkingDay(usize, bool),
    Prompt(PromptKind, text_editor::Action),
    /// Put the built-in prompt back in the editor.
    ResetPrompt(PromptKind),
    Secret(SecretField, Typed),
    /// Submit the API key field: opens the password pop-up.
    SaveKey,
    /// Submit the password pop-up.
    SubmitPassword,
    CancelPassword,
    /// Open the unlock prompt.
    Unlock,
    Lock,
    /// Open the replace-key form.
    ShowReplace,
    /// First click on Remove key: ask for confirmation.
    AskRemove,
    ConfirmRemove,
    /// Close whichever key form or confirmation is open.
    CancelKey,
    AppIcon(AppIcon),
}

/// Which endpoint a field, button or request is for: the main LLM, or the
/// optional decision model used for recommended tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Main,
    Decision,
}

/// What `main.rs` must do.
#[derive(Debug, Clone)]
pub enum Action {
    None,
    /// Close the overlay (the close button or Escape).
    Close,
    /// Fetch the endpoint's model list using these endpoint fields and the
    /// current key (for `Which::Decision`, the key only if it is on the main
    /// endpoint's host). The result arrives as `SettingsData::models`.
    ListModels(LlmSettings, Which),
    /// Save these settings (endpoint, schedule, capacity, prompts).
    Save(Settings),
    /// Send a test request using these endpoint fields and the current key
    /// (same rule as `ListModels`).
    Test(LlmSettings, Which),
    /// Encrypt and store a new API key (replacing any existing one).
    SetKey {
        api_key: SecretString,
        password: SecretString,
    },
    RemoveKey,
    Lock,
    /// Show the unlock prompt.
    ShowUnlock,
    /// The password pop-up opened: focus its field (`PASSWORD_INPUT_ID`).
    FocusPassword,
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
    /// Result of the last key operation (set, remove, unlock),
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

/// An app icon choice with a `Display` impl, for `pick_list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IconChoice(AppIcon);

const ICON_CHOICES: [IconChoice; 3] = [
    IconChoice(AppIcon::System),
    IconChoice(AppIcon::Light),
    IconChoice(AppIcon::Dark),
];

impl fmt::Display for IconChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            AppIcon::System => "Match system",
            AppIcon::Light => "Light",
            AppIcon::Dark => "Dark",
        })
    }
}

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

/// One endpoint's fields as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Endpoint {
    protocol: Protocol,
    base_url: String,
    model: String,
}

impl Endpoint {
    fn from_llm(llm: &LlmSettings) -> Self {
        Endpoint {
            protocol: llm.protocol,
            base_url: llm.base_url.clone(),
            model: llm.model.clone(),
        }
    }

    fn llm(&self) -> LlmSettings {
        LlmSettings {
            protocol: self.protocol,
            base_url: self.base_url.trim().to_string(),
            model: self.model.trim().to_string(),
        }
    }
}

/// The editable copy of the non-secret settings.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Form {
    main: Endpoint,
    decision: Endpoint,
    week_start: Weekday,
    summary_day: Weekday,
    summary_time: String,
    plan_day: Weekday,
    plan_time: String,
    /// Indexed Monday = 0.
    working_days: [bool; 7],
    /// Decimal hours as typed, e.g. `5.5`.
    focus_hours: String,
    /// Parked: carried through unchanged.
    notifications: bool,
    app_icon: AppIcon,
    /// The prompts as shown and edited.
    summary_prompt: String,
    plan_prompt: String,
    /// The summary prompt as first shown, and the stored fields it came
    /// from, so an untouched prompt saves back exactly as it was.
    summary_shown: String,
    summary_stored: (String, String),
}

impl Default for Form {
    fn default() -> Self {
        Form::from_settings(&Settings::default())
    }
}

impl Form {
    fn from_settings(s: &Settings) -> Self {
        let sched = &s.schedule;
        let mut summary_shown = match s.summary_prompt.trim() {
            "" => DEFAULT_SUMMARY_PROMPT.to_string(),
            custom => custom.to_string(),
        };
        let extra = s.summary_instructions.trim();
        if !extra.is_empty() {
            summary_shown.push_str("\n\n");
            summary_shown.push_str(extra);
        }
        Form {
            main: Endpoint::from_llm(&s.llm),
            decision: Endpoint::from_llm(&s.decision_llm),
            week_start: sched.week_start,
            summary_day: sched.summary_day,
            summary_time: format_time(sched.summary_time),
            plan_day: sched.plan_day,
            plan_time: format_time(sched.plan_time),
            working_days: WEEK.map(|d| sched.working_days.contains(&d)),
            focus_hours: format_hours(s.focus_minutes_per_day),
            notifications: s.notifications,
            app_icon: s.app_icon,
            summary_prompt: summary_shown.clone(),
            plan_prompt: match s.plan_prompt.trim() {
                "" => DEFAULT_PLAN_PROMPT.to_string(),
                custom => custom.to_string(),
            },
            summary_shown,
            summary_stored: (s.summary_prompt.clone(), s.summary_instructions.clone()),
        }
    }

    /// The settings this form describes, or `None` if a time or the focus
    /// hours are invalid.
    fn to_settings(&self) -> Option<Settings> {
        let (summary_prompt, summary_instructions) = if self.summary_prompt == self.summary_shown {
            self.summary_stored.clone()
        } else {
            (
                stored_prompt(&self.summary_prompt, DEFAULT_SUMMARY_PROMPT),
                String::new(),
            )
        };
        Some(Settings {
            llm: self.main.llm(),
            decision_llm: self.decision.llm(),
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
            summary_instructions,
            focus_minutes_per_day: parse_hours(&self.focus_hours)?,
            notifications: self.notifications,
            app_icon: self.app_icon,
            summary_prompt,
            plan_prompt: stored_prompt(&self.plan_prompt, DEFAULT_PLAN_PROMPT),
        })
    }

    fn endpoint(&self, which: Which) -> &Endpoint {
        match which {
            Which::Main => &self.main,
            Which::Decision => &self.decision,
        }
    }

    fn endpoint_mut(&mut self, which: Which) -> &mut Endpoint {
        match which {
            Which::Main => &mut self.main,
            Which::Decision => &mut self.decision,
        }
    }
}

/// A prompt as stored: empty when it is the built-in one.
fn stored_prompt(text: &str, default: &str) -> String {
    let text = text.trim();
    if text == default.trim() {
        String::new()
    } else {
        text.to_string()
    }
}

/// Which key form or confirmation is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum KeyMode {
    #[default]
    Idle,
    Replace,
    ConfirmRemove,
}

/// The secret fields. `Debug` never prints them.
#[derive(Default)]
struct Secrets {
    api_key: String,
    password: String,
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
        }
    }

    fn wipe_all(&mut self) {
        wipe(&mut self.api_key);
        wipe(&mut self.password);
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

/// Checks a new password.
fn check_password(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(format!(
            "The password needs at least {MIN_PASSWORD_LEN} characters."
        ));
    }
    Ok(())
}

pub const PASSWORD_INPUT_ID: &str = "settings-key-password";

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
    listed_for: Option<(Which, Protocol, String)>,
    /// The section whose Test button was pressed last; its result shows there.
    tested: Which,
    /// The password pop-up is open (after Save key).
    password_prompt: bool,
    summary_editor: text_editor::Content,
    plan_editor: text_editor::Content,
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
            tested: Which::Main,
            password_prompt: false,
            summary_editor: text_editor::Content::new(),
            plan_editor: text_editor::Content::new(),
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
            self.summary_editor = text_editor::Content::with_text(&self.form.summary_prompt);
            self.plan_editor = text_editor::Content::with_text(&self.form.plan_prompt);
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

    fn can_test(&self, which: Which) -> bool {
        self.form.endpoint(which).llm().is_configured() && !self.testing
    }

    fn can_list_models(&self, which: Which) -> bool {
        !self.form.endpoint(which).base_url.trim().is_empty() && !self.listing_models
    }

    fn models_state(&self, which: Which) -> ModelsState<'_> {
        let e = self.form.endpoint(which);
        match &self.listed_for {
            Some((w, protocol, base_url))
                if *w == which && *protocol == e.protocol && *base_url == e.base_url.trim() => {}
            // The list belongs to the other section or an endpoint the form
            // no longer names.
            Some(_) => return ModelsState::Hidden,
            None => {}
        }
        if self.listing_models {
            return ModelsState::Busy;
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
            Message::Protocol(w, p) => form.endpoint_mut(w).protocol = p,
            Message::BaseUrl(w, s) => form.endpoint_mut(w).base_url = s,
            Message::Model(w, s) | Message::PickModel(w, s) => form.endpoint_mut(w).model = s,
            Message::ClearDecision => form.decision = Endpoint::default(),
            Message::FocusHours(s) => form.focus_hours = s,
            Message::UseOllama(w) => {
                let e = form.endpoint_mut(w);
                e.protocol = Protocol::OpenAi;
                e.base_url = OLLAMA_BASE_URL.into();
                return self.update(Message::LoadModels(w));
            }
            Message::LoadModels(w) => {
                if self.can_list_models(w) {
                    let llm = self.form.endpoint(w).llm();
                    self.listed_for = Some((w, llm.protocol, llm.base_url.clone()));
                    return Action::ListModels(llm, w);
                }
            }
            Message::Close => {
                // Typed but unsubmitted secrets don't outlive the overlay.
                self.close_key_form();
                return Action::Close;
            }
            Message::WeekStart(d) => form.week_start = d,
            Message::AppIcon(choice) => form.app_icon = choice,
            Message::SummaryDay(d) => form.summary_day = d,
            Message::SummaryTime(s) => form.summary_time = s,
            Message::PlanDay(d) => form.plan_day = d,
            Message::PlanTime(s) => form.plan_time = s,
            Message::WorkingDay(i, on) => {
                if let Some(day) = form.working_days.get_mut(i) {
                    *day = on;
                }
            }
            Message::Prompt(kind, action) => {
                let editor = match kind {
                    PromptKind::Summary => &mut self.summary_editor,
                    PromptKind::Plan => &mut self.plan_editor,
                };
                editor.perform(action);
                let text = editor.text();
                match kind {
                    PromptKind::Summary => form.summary_prompt = text,
                    PromptKind::Plan => form.plan_prompt = text,
                }
            }
            Message::ResetPrompt(kind) => {
                let (text, editor) = match kind {
                    PromptKind::Summary => (&mut form.summary_prompt, &mut self.summary_editor),
                    PromptKind::Plan => (&mut form.plan_prompt, &mut self.plan_editor),
                };
                let default = match kind {
                    PromptKind::Summary => DEFAULT_SUMMARY_PROMPT,
                    PromptKind::Plan => DEFAULT_PLAN_PROMPT,
                };
                *text = default.to_string();
                *editor = text_editor::Content::with_text(default);
            }
            Message::Save => {
                if let Some(settings) = self.savable() {
                    return Action::Save(settings);
                }
            }
            Message::Test(w) => {
                if self.can_test(w) {
                    self.tested = w;
                    return Action::Test(self.form.endpoint(w).llm(), w);
                }
            }
            Message::Secret(field, Typed(value)) => {
                let slot = self.secrets.field(field);
                wipe(slot);
                *slot = value;
            }
            Message::SaveKey => {
                if self.secrets.api_key.trim().is_empty() {
                    self.key_error = Some("Enter the API key.".into());
                    return Action::None;
                }
                self.key_error = None;
                self.password_prompt = true;
                return Action::FocusPassword;
            }
            Message::SubmitPassword => return self.save_key(),
            Message::CancelPassword => {
                self.password_prompt = false;
                self.key_error = None;
                wipe(&mut self.secrets.password);
            }
            Message::Unlock => return Action::ShowUnlock,
            Message::Lock => {
                self.close_key_form();
                return Action::Lock;
            }
            Message::ShowReplace => self.open_key_form(KeyMode::Replace),
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
        self.password_prompt = false;
        self.secrets.wipe_all();
    }

    fn save_key(&mut self) -> Action {
        if self.secrets.api_key.trim().is_empty() {
            self.password_prompt = false;
            return Action::None;
        }
        if let Err(e) = check_password(&self.secrets.password) {
            self.key_error = Some(e);
            return Action::None;
        }
        let api_key = SecretString::from(self.secrets.api_key.trim().to_string());
        let password = take_secret(&mut self.secrets.password);
        self.close_key_form();
        Action::SetKey { api_key, password }
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
            self.decision_section(),
            self.schedule_section(),
            self.capacity_section(),
            self.prompts_section(),
            self.appearance_section(),
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

        let page = column![
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
        ];
        if !self.password_prompt {
            return page.into();
        }
        stack![page, opaque(self.password_view())].into()
    }

    /// The pop-up that asks for the password that encrypts the key.
    fn password_view(&self) -> Element<'_, Message> {
        let field = input(
            &format!("At least {MIN_PASSWORD_LEN} characters"),
            &self.secrets.password,
        )
        .id(PASSWORD_INPUT_ID)
        .secure(true)
        .on_input(|s| Message::Secret(SecretField::Password, Typed(s)))
        .on_submit(Message::SubmitPassword);
        let mut card = column![
            text("Choose a password").size(theme::TEXT_HEADING),
            small_muted("It encrypts the key. It can't be recovered if you forget it."),
            field,
        ]
        .spacing(theme::SPACING);
        if let Some(e) = &self.key_error {
            card = card.push(text(e).size(theme::TEXT_SMALL).style(theme::danger));
        }
        card = card.push(
            row![
                primary("Save", Message::SubmitPassword),
                button(text("Cancel").size(theme::TEXT_BODY))
                    .padding([8, 14])
                    .style(theme::text_button)
                    .on_press(Message::CancelPassword),
            ]
            .spacing(theme::SPACING / 2.0),
        );
        container(
            container(card)
                .padding(theme::SPACING * 2.0)
                .max_width(380)
                .style(theme::overlay_panel),
        )
        .center(Fill)
        .style(theme::backdrop)
        .into()
    }

    fn endpoint_section(&self) -> Element<'_, Message> {
        section("LLM", self.endpoint_fields(Which::Main))
    }

    fn decision_section(&self) -> Element<'_, Message> {
        let d = &self.form.decision;
        let blank = d.base_url.trim().is_empty() && d.model.trim().is_empty();
        let clear = button(text("Clear").size(theme::TEXT_SMALL))
            .padding([4, 10])
            .style(theme::secondary_button)
            .on_press_maybe((!blank).then_some(Message::ClearDecision));
        let about = row![
            container(small_muted(
                "Optional. Used for Recommended tasks instead of the LLM above, such as a \
                 reasoning model on a local Ollama. When it isn't set, or can't be reached, the \
                 LLM above is used. It gets the API key only on the same host as the LLM above."
            ))
            .width(Fill),
            clear
        ]
        .spacing(theme::SPACING)
        .align_y(Center);
        section(
            "Decision model",
            column![about, self.endpoint_fields(Which::Decision)].spacing(theme::SPACING),
        )
    }

    /// Protocol, base URL, model (with its list) and Test for one endpoint.
    fn endpoint_fields(&self, which: Which) -> Element<'_, Message> {
        let f = self.form.endpoint(which);
        let protocol = labelled(
            "Protocol",
            pick_list(Protocol::ALL, Some(f.protocol), move |p| {
                Message::Protocol(which, p)
            })
            .padding([8, 12])
            .text_size(theme::TEXT_BODY)
            .width(Fill)
            .style(picker),
        );
        let base_url = labelled(
            "Base URL",
            input(f.protocol.example_base_url(), &f.base_url)
                .on_input(move |s| Message::BaseUrl(which, s)),
        );
        let model_placeholder = match (which, f.protocol) {
            (Which::Decision, Protocol::OpenAi) => "Model name, such as deepseek-r1 or qwen3",
            (Which::Main, Protocol::OpenAi) => "Model name, such as llama3.2",
            (_, Protocol::Anthropic) => "Model name, such as claude-opus-5-5",
        };
        let listing_here = self.listing_models
            && self
                .listed_for
                .as_ref()
                .is_some_and(|(w, _, _)| *w == which);
        let load_label = if listing_here {
            "Loading…"
        } else {
            "Load models"
        };
        let load = button(text(load_label).size(theme::TEXT_BODY))
            .padding([8, 14])
            .style(theme::secondary_button)
            .on_press_maybe(
                self.can_list_models(which)
                    .then_some(Message::LoadModels(which)),
            );
        let model = labelled(
            "Model",
            row![
                input(model_placeholder, &f.model).on_input(move |s| Message::Model(which, s)),
                load
            ]
            .spacing(theme::SPACING / 2.0)
            .align_y(Center),
        );
        let models: Element<'_, Message> = match self.models_state(which) {
            ModelsState::Hidden => Space::new().into(),
            ModelsState::Busy => small_muted("Asking the endpoint for its models…"),
            ModelsState::Empty => {
                small_muted("The endpoint didn't list any models. Type the model name above.")
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
                    pick_list(ids, selected, move |m| Message::PickModel(which, m))
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
        let local = is_local_endpoint(&f.base_url);
        let hint_text = match f.protocol {
            _ if local => {
                "A local endpoint: no API key needed. Reasoning models work; their thinking is hidden."
            }
            Protocol::OpenAi => {
                "Any OpenAI-compatible endpoint works, such as Ollama at http://localhost:11434/v1."
            }
            Protocol::Anthropic => {
                "Anthropic's Messages API, usually at https://api.anthropic.com."
            }
        };
        let use_ollama = button(text("Use local Ollama").size(theme::TEXT_SMALL))
            .padding([4, 10])
            .style(theme::secondary_button)
            .on_press_maybe(
                (f.protocol != Protocol::OpenAi || f.base_url.trim() != OLLAMA_BASE_URL)
                    .then_some(Message::UseOllama(which)),
            );
        let hint = row![container(small_muted(hint_text)).width(Fill), use_ollama]
            .spacing(theme::SPACING)
            .align_y(Center);

        let testing_here = self.testing && self.tested == which;
        let test_label = if testing_here {
            "Testing…"
        } else {
            "Test connection"
        };
        let test = button(text(test_label).size(theme::TEXT_BODY))
            .padding([8, 18])
            .style(theme::secondary_button)
            .on_press_maybe(self.can_test(which).then_some(Message::Test(which)));
        let mut test_row = row![test].spacing(theme::SPACING).align_y(Center);
        let result = if self.tested == which && !self.testing {
            self.test_result.as_ref()
        } else {
            None
        };
        match result {
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
            None if which == Which::Main
                && !self.testing
                && self.key_state == KeyState::Locked
                && !local =>
            {
                test_row = test_row.push(small_muted("The key is locked. Unlock it first."))
            }
            None => {}
        }

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
        .spacing(theme::SPACING)
        .into()
    }

    fn key_section(&self) -> Element<'_, Message> {
        let mut body = column![].spacing(theme::SPACING);
        let state = |label: &'static str| {
            container(text(label).size(theme::TEXT_SMALL))
                .padding(theme::chip_padding())
                .style(theme::chip)
        };

        match (self.key_state, self.key_mode) {
            (_, KeyMode::ConfirmRemove) => {
                body = body.push(
                    row![
                        text("Remove the key?").size(theme::TEXT_BODY),
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
            (KeyState::NoKey, _) => body = body.push(self.key_input(false)),
            (_, KeyMode::Replace) => body = body.push(self.key_input(true)),
            (KeyState::Locked, KeyMode::Idle) => {
                body = body.push(
                    row![
                        state("Saved, locked"),
                        secondary("Unlock", Message::Unlock),
                        secondary("Replace", Message::ShowReplace),
                        Space::new().width(Fill),
                        remove_button(),
                    ]
                    .spacing(theme::SPACING)
                    .align_y(Center),
                );
            }
            (KeyState::Unlocked, KeyMode::Idle) => {
                body = body.push(
                    row![
                        state("Saved, unlocked"),
                        secondary("Lock", Message::Lock),
                        secondary("Replace", Message::ShowReplace),
                        Space::new().width(Fill),
                        remove_button(),
                    ]
                    .spacing(theme::SPACING)
                    .align_y(Center),
                );
            }
        }

        if self.password_prompt {
            // The pop-up shows the error.
        } else if let Some(e) = &self.key_error {
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

        section("API key", body)
    }

    /// The API key field and Save key (Enter works too).
    fn key_input(&self, cancellable: bool) -> Element<'_, Message> {
        let mut line = row![
            secret_input("sk-…", &self.secrets.api_key, SecretField::ApiKey)
                .on_submit(Message::SaveKey),
            primary("Save key", Message::SaveKey),
        ]
        .spacing(theme::SPACING / 2.0)
        .align_y(Center);
        if cancellable {
            line = line.push(cancel_button());
        }
        line.into()
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
            "Recommended tasks",
            day_picker(f.plan_day, Message::PlanDay),
            &f.plan_time,
            Message::PlanTime,
        );

        section(
            "Schedule",
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
                small_muted("The Week tab compares estimates against this."),
            ]
            .spacing(theme::SPACING)
            .align_y(Center),
        ]
        .spacing(theme::SPACING / 3.0);
        if parse_hours(&f.focus_hours).is_none() {
            hours = hours.push(
                text("Enter hours between 0 and 24, such as 6 or 5.5.")
                    .size(theme::TEXT_SMALL)
                    .style(theme::danger),
            );
        }
        section(
            "Capacity",
            column![
                column![text("Working days").size(theme::TEXT_SMALL), days]
                    .spacing(theme::SPACING / 2.0),
                hours,
            ]
            .spacing(theme::SPACING * 1.25),
        )
    }

    fn prompts_section(&self) -> Element<'_, Message> {
        let editor = |label: &'static str, kind: PromptKind| {
            let (content, current, default) = match kind {
                PromptKind::Summary => (
                    &self.summary_editor,
                    &self.form.summary_prompt,
                    DEFAULT_SUMMARY_PROMPT,
                ),
                PromptKind::Plan => (
                    &self.plan_editor,
                    &self.form.plan_prompt,
                    DEFAULT_PLAN_PROMPT,
                ),
            };
            let reset = button(text("Reset").size(theme::TEXT_SMALL))
                .padding([2, 8])
                .style(theme::text_button)
                .on_press_maybe(
                    (current.trim() != default.trim()).then_some(Message::ResetPrompt(kind)),
                );
            column![
                row![
                    text(label).size(theme::TEXT_SMALL),
                    Space::new().width(Fill),
                    reset
                ]
                .align_y(Center),
                text_editor(content)
                    .on_action(move |a| Message::Prompt(kind, a))
                    .height(220)
                    .padding(10)
                    .size(theme::TEXT_SMALL)
                    .style(theme::editor),
            ]
            .spacing(theme::SPACING / 3.0)
        };
        section(
            "Prompts",
            column![
                editor("Work summary", PromptKind::Summary),
                editor("Recommended tasks", PromptKind::Plan),
            ]
            .spacing(theme::SPACING * 1.25),
        )
    }

    fn appearance_section(&self) -> Element<'_, Message> {
        let icon = pick_list(
            ICON_CHOICES,
            Some(IconChoice(self.form.app_icon)),
            |c: IconChoice| Message::AppIcon(c.0),
        )
        .padding([8, 12])
        .text_size(theme::TEXT_BODY)
        .width(180)
        .style(picker);
        section("Appearance", labelled("App icon", icon))
    }

    fn data_section(&self) -> Element<'_, Message> {
        section(
            "Data folder",
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
            text("Use 24-hour time, such as 16:00.")
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

/// Ollama's OpenAI-compatible endpoint on its default port.
const OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";

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
    /// Close without unlocking.
    Skip,
}

/// The password prompt, opened from the lock icon.
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
        let skip = button(text("Cancel").size(theme::TEXT_SMALL))
            .padding([4, 8])
            .style(theme::text_button)
            .on_press(UnlockMessage::Skip);

        let mut card = column![
            text("Unlock the API key").size(theme::TEXT_HEADING),
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
                .max_width(380)
                .style(theme::overlay_panel),
        )
        .center(Fill)
        .style(theme::backdrop)
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
        v.update(Message::BaseUrl(
            Which::Main,
            "http://localhost:11434/v1".into(),
        ));
        typed(&mut v, SecretField::ApiKey, "sk-abc");
        v.load(data(Settings::default(), KeyState::NoKey));
        assert_eq!(v.form.main.base_url, "http://localhost:11434/v1");
        assert_eq!(v.secrets.api_key, "sk-abc");
    }

    #[test]
    fn load_resets_form_when_settings_change() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::BaseUrl(Which::Main, "typed".into()));
        let mut s = Settings::default();
        s.llm.model = "gpt-4o".into();
        v.load(data(s, KeyState::NoKey));
        assert_eq!(v.form.main.base_url, "");
        assert_eq!(v.form.main.model, "gpt-4o");
    }

    #[test]
    fn save_emits_full_settings_with_parsed_times() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::BaseUrl(
            Which::Main,
            " https://api.openai.com/v1 ".into(),
        ));
        v.update(Message::Model(Which::Main, "gpt-4o".into()));
        v.update(Message::SummaryDay(Weekday::Thursday));
        v.update(Message::SummaryTime("9:30".into()));
        v.update(Message::PlanTime("07:05".into()));
        v.update(Message::WorkingDay(4, false));
        v.update(Message::WorkingDay(5, true));
        v.update(Message::ResetPrompt(PromptKind::Plan));
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
        assert_eq!(s.summary_instructions, "");
        assert_eq!(s.plan_prompt, "", "the built-in prompt is stored as empty");
    }

    #[test]
    fn app_icon_choice_saves() {
        let mut v = loaded(KeyState::NoKey);
        assert_eq!(v.form.app_icon, AppIcon::System);
        v.update(Message::AppIcon(AppIcon::Dark));
        assert!(v.is_dirty());
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.app_icon, AppIcon::Dark);
    }

    #[test]
    fn save_does_nothing_when_unchanged() {
        let mut v = loaded(KeyState::NoKey);
        assert!(matches!(v.update(Message::Save), Action::None));
    }

    #[test]
    fn invalid_time_blocks_save() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::Model(Which::Main, "m".into()));
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
    fn save_key_asks_for_a_password_then_emits() {
        let mut v = loaded(KeyState::NoKey);
        assert!(matches!(v.update(Message::SaveKey), Action::None));
        assert!(v.key_error.is_some(), "no key typed");
        assert!(!v.password_prompt);

        typed(&mut v, SecretField::ApiKey, " sk-abc ");
        assert!(matches!(v.update(Message::SaveKey), Action::FocusPassword));
        assert!(v.password_prompt);
        typed(&mut v, SecretField::Password, "short");
        assert!(matches!(v.update(Message::SubmitPassword), Action::None));
        assert!(v.key_error.is_some());
        assert!(v.password_prompt, "stays open to fix it");

        typed(&mut v, SecretField::Password, "correct horse");
        let Action::SetKey { api_key, password } = v.update(Message::SubmitPassword) else {
            panic!("expected SetKey");
        };
        assert_eq!(api_key.expose_secret(), "sk-abc");
        assert_eq!(password.expose_secret(), "correct horse");
        assert!(v.secrets.api_key.is_empty());
        assert!(v.secrets.password.is_empty());
        assert!(!v.password_prompt);
        assert!(v.key_error.is_none());

        // Cancel keeps the typed key but drops the password.
        typed(&mut v, SecretField::ApiKey, "sk-2");
        v.update(Message::SaveKey);
        typed(&mut v, SecretField::Password, "something long");
        v.update(Message::CancelPassword);
        assert!(!v.password_prompt);
        assert!(v.secrets.password.is_empty());
        assert_eq!(v.secrets.api_key, "sk-2");
    }

    #[test]
    fn prompts_edit_reset_and_save() {
        let mut v = loaded(KeyState::NoKey);
        assert_eq!(v.form.summary_prompt, DEFAULT_SUMMARY_PROMPT);
        v.update(Message::Prompt(
            PromptKind::Summary,
            text_editor::Action::Move(text_editor::Motion::DocumentStart),
        ));
        v.update(Message::Prompt(
            PromptKind::Summary,
            text_editor::Action::Edit(text_editor::Edit::Paste(std::sync::Arc::new(
                "Be brief. ".to_string(),
            ))),
        ));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert!(s.summary_prompt.starts_with("Be brief. You write"));
        assert_eq!(s.plan_prompt, "");
        v.update(Message::ResetPrompt(PromptKind::Summary));
        assert!(
            matches!(v.update(Message::Save), Action::None),
            "back to unchanged"
        );
    }

    #[test]
    fn round2_summary_instructions_are_folded_into_the_prompt() {
        let s = Settings {
            summary_instructions: "Short.".into(),
            ..Settings::default()
        };
        let mut v = SettingsView::new();
        v.load(data(s.clone(), KeyState::NoKey));
        assert!(v.form.summary_prompt.ends_with("\n\nShort."));
        assert!(!v.is_dirty());
        assert!(matches!(v.update(Message::Save), Action::None));
        // Any other change keeps them as they are.
        v.update(Message::Model(Which::Main, "m".into()));
        let Action::Save(saved) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(saved.summary_instructions, "Short.");
        assert_eq!(saved.summary_prompt, "");
        // Editing the prompt folds them in for good.
        v.update(Message::Prompt(
            PromptKind::Summary,
            text_editor::Action::Edit(text_editor::Edit::Insert('!')),
        ));
        let Action::Save(saved) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(saved.summary_instructions, "");
        assert!(saved.summary_prompt.contains("Short."));
    }

    #[test]
    fn use_ollama_fills_the_endpoint_and_lists_models() {
        let mut v = loaded(KeyState::Locked);
        v.update(Message::Protocol(Which::Main, Protocol::Anthropic));
        let Action::ListModels(llm, Which::Main) = v.update(Message::UseOllama(Which::Main)) else {
            panic!("expected ListModels");
        };
        assert_eq!(llm.protocol, Protocol::OpenAi);
        assert_eq!(llm.base_url, OLLAMA_BASE_URL);
        assert_eq!(v.form.main.base_url, OLLAMA_BASE_URL);
        assert_eq!(v.form.main.protocol, Protocol::OpenAi);
    }

    #[test]
    fn decision_model_saves_tests_and_clears_on_its_own() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::BaseUrl(
            Which::Main,
            "https://api.openai.com/v1".into(),
        ));
        v.update(Message::Model(Which::Main, "gpt-4o".into()));
        let Action::ListModels(llm, Which::Decision) =
            v.update(Message::UseOllama(Which::Decision))
        else {
            panic!("expected ListModels for the decision model");
        };
        assert_eq!(llm.base_url, OLLAMA_BASE_URL);
        // The list belongs to the decision section only.
        assert_eq!(v.models_state(Which::Main), ModelsState::Hidden);
        v.update(Message::PickModel(Which::Decision, "deepseek-r1:8b".into()));
        let Action::Test(llm, Which::Decision) = v.update(Message::Test(Which::Decision)) else {
            panic!("expected Test for the decision model");
        };
        assert_eq!(llm.model, "deepseek-r1:8b");

        let Action::Save(saved) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(saved.llm.model, "gpt-4o");
        assert_eq!(saved.decision_llm.base_url, OLLAMA_BASE_URL);
        assert_eq!(saved.decision_llm.model, "deepseek-r1:8b");

        v.load(data(saved, KeyState::NoKey));
        v.update(Message::ClearDecision);
        let Action::Save(cleared) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert!(!cleared.decision_llm.is_configured());
        assert_eq!(cleared.llm.model, "gpt-4o");
    }

    #[test]
    fn test_emits_current_fields() {
        let mut v = loaded(KeyState::NoKey);
        assert!(matches!(v.update(Message::Test(Which::Main)), Action::None));
        v.update(Message::BaseUrl(
            Which::Main,
            "http://localhost:11434/v1".into(),
        ));
        v.update(Message::Model(Which::Main, "llama3.2".into()));
        let Action::Test(llm, Which::Main) = v.update(Message::Test(Which::Main)) else {
            panic!("expected Test");
        };
        assert_eq!(llm.base_url, "http://localhost:11434/v1");
        assert_eq!(llm.model, "llama3.2");
        // Not while a test is running.
        let mut d = data(Settings::default(), KeyState::NoKey);
        d.testing = true;
        v.load(d);
        assert!(matches!(v.update(Message::Test(Which::Main)), Action::None));
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
        assert!(matches!(v.update(Message::Unlock), Action::ShowUnlock));
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
        v.update(Message::Protocol(Which::Main, Protocol::Anthropic));
        v.update(Message::BaseUrl(
            Which::Main,
            "https://api.anthropic.com".into(),
        ));
        v.update(Message::Model(Which::Main, "claude-x".into()));
        v.update(Message::FocusHours(" 5.5 ".into()));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.llm.protocol, Protocol::Anthropic);
        assert_eq!(s.focus_minutes_per_day, 330);
        // Saved settings come back from main.rs: the form matches them and
        // there is nothing left to save.
        v.load(data(s.clone(), KeyState::NoKey));
        assert_eq!(v.form.main.protocol, Protocol::Anthropic);
        assert_eq!(v.form.focus_hours, "5.5");
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
        assert_eq!(v.form.main.protocol, Protocol::OpenAi);
        assert_eq!(v.form.focus_hours, "6");
        assert!(!v.is_dirty());
        v.update(Message::FocusHours("6.5".into()));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.llm, round1.llm);
        assert_eq!(s.schedule, round1.schedule);
        assert_eq!(s.focus_minutes_per_day, DEFAULT_FOCUS_MINUTES + 30);
        assert!(!s.notifications);
        assert_eq!(s.summary_instructions, "Short.");
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
        assert!(matches!(
            v.update(Message::LoadModels(Which::Main)),
            Action::None
        ));
        v.update(Message::Protocol(Which::Main, Protocol::Anthropic));
        v.update(Message::BaseUrl(
            Which::Main,
            " https://api.anthropic.com ".into(),
        ));
        v.update(Message::Model(Which::Main, "typed".into()));
        let Action::ListModels(llm, Which::Main) = v.update(Message::LoadModels(Which::Main))
        else {
            panic!("expected ListModels");
        };
        assert_eq!(llm.protocol, Protocol::Anthropic);
        assert_eq!(llm.base_url, "https://api.anthropic.com");
        assert_eq!(llm.model, "typed");
        // Not while a fetch is in flight.
        let mut d = data(Settings::default(), KeyState::NoKey);
        d.listing_models = true;
        v.load(d);
        assert_eq!(v.models_state(Which::Main), ModelsState::Busy);
        assert!(matches!(
            v.update(Message::LoadModels(Which::Main)),
            Action::None
        ));
    }

    #[test]
    fn models_result_handling() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::BaseUrl(
            Which::Main,
            "http://localhost:11434/v1".into(),
        ));
        assert_eq!(v.models_state(Which::Main), ModelsState::Hidden);
        v.update(Message::LoadModels(Which::Main));

        let with = |models| {
            let mut d = data(Settings::default(), KeyState::NoKey);
            d.models = Some(models);
            d
        };
        v.load(with(Ok(vec![])));
        assert_eq!(v.models_state(Which::Main), ModelsState::Empty);

        v.load(with(Err("404 Not Found".into())));
        assert_eq!(
            v.models_state(Which::Main),
            ModelsState::Error("404 Not Found")
        );

        let ids = vec!["llama3.2".to_string(), "qwen3".to_string()];
        v.load(with(Ok(ids.clone())));
        assert_eq!(v.models_state(Which::Main), ModelsState::List(&ids));
        v.update(Message::PickModel(Which::Main, "qwen3".into()));
        assert_eq!(v.form.main.model, "qwen3");
        // Free text still works after picking.
        v.update(Message::Model(Which::Main, "qwen3:8b".into()));
        assert_eq!(v.form.main.model, "qwen3:8b");
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.llm.model, "qwen3:8b");

        // A list for another endpoint is hidden.
        v.update(Message::Protocol(Which::Main, Protocol::Anthropic));
        assert_eq!(v.models_state(Which::Main), ModelsState::Hidden);
        v.update(Message::Protocol(Which::Main, Protocol::OpenAi));
        assert_eq!(v.models_state(Which::Main), ModelsState::List(&ids));
    }

    #[test]
    fn close_emits_close_and_keeps_edits() {
        let mut v = loaded(KeyState::NoKey);
        assert!(matches!(v.update(Message::Close), Action::Close));
        v.update(Message::Model(Which::Main, "unsaved".into()));
        assert!(matches!(v.update(Message::Close), Action::Close));
        v.load(data(Settings::default(), KeyState::NoKey));
        assert_eq!(v.form.main.model, "unsaved");
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
