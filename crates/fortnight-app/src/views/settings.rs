//! Settings overlay and the unlock prompt.
//!
//! Settings opens as an overlay panel over the current tab. It is one
//! scrollable column under a fixed header that holds the title, the single
//! Save button (it saves the whole form: endpoint and protocol, schedule,
//! capacity, the two prompts and the appearance) and a close button. Closing with unsaved
//! changes is allowed: the header says "Unsaved changes", and the edits stay
//! in the form until the saved settings change.
//!
//! Models: two blocks of label/field rows, Model (everything) and
//! Recommendations (off unless "Use a different model" is ticked; then it
//! is `Settings::decision_llm`). Each block has Provider (Ollama, which is
//! keyless; OpenAI-compatible; Anthropic), URL, Model with a Load/Choose
//! list, API key (not for Ollama) and Test. No help text, by request.
//!
//! The API key controls act immediately and are not part of Save. Each
//! block has its own key; Save (or Enter) asks for a password in a small
//! pop-up, which encrypts the key, and both keys share that password.
//! Unlocking is done from the lock icon in the top bar (the Unlock button
//! here opens the same prompt); so is locking.
//!
//! The prompts: the system prompts for the Work summary and Recommended
//! tasks, shown in full (the built-in one until changed) with Reset. A
//! prompt equal to the built-in one is saved as empty, so it follows future
//! built-in changes. Round 2's "summary instructions" are shown folded into
//! the summary prompt and are folded in for good once that prompt is saved.
//!
//! Picking a provider fills in its default URL (unless one was typed);
//! Ollama also loads its model list. Ollama on this machine is checked at
//! launch and when Settings opens (`SettingsData::ollama`): an Ollama block
//! on the default address shows Running / Not running and lists its models
//! without Load, tagged "thinking" or "decision". Decision models (Ollama's
//! Jev-style `/v1/systemone`) are offered only for Recommendations; picking
//! one saves that block as `Protocol::OllamaDecision`. On a first run with
//! Ollama running, Model starts on Ollama with its first chat model. Reasoning models (DeepSeek-R1, Qwen3,
//! gpt-oss) work like any other: the client drops their thinking.
//!
//! Secrets (API key, password) live in the form as `String` only while
//! typed. They are moved into a `SecretString` when an action is emitted,
//! and wiped otherwise. `Message` prints them as `***` in `Debug` output.
//!
//! Skipped for the prototype: a "working…" indicator while Argon2 runs,
//! selectable data-folder text (iced 0.14 has no selectable text widget),
//! and a confirmation before closing with unsaved changes.

use fortnight_core::domain::{Accent, AppIcon, LlmSettings, Protocol, Schedule, Settings};
use fortnight_core::ollama::{self, DEFAULT_BASE_URL as OLLAMA_BASE_URL, ModelKind, OllamaModel};
use fortnight_core::parse::{format_hours, format_time, parse_hours, parse_time};
use fortnight_core::report::{
    DEFAULT_PLAN_PROMPT, DEFAULT_SUMMARY_PROMPT, WEEKDAYS, short_weekday, weekday_name,
};
use fortnight_core::vault::MIN_PASSWORD_LEN;
use iced::widget::{
    Space, button, checkbox, column, container, opaque, operation, pick_list, row, rule,
    scrollable, stack, text, text_editor, text_input, tooltip,
};
use iced::{Background, Border, Center, Color, Element, Fill, Theme};
use jiff::civil::Weekday;
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
    /// The decision model's own API key.
    DecisionKey,
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
    Provider(Which, Provider),
    BaseUrl(Which, String),
    Model(Which, String),
    /// Ask the endpoint for its model list.
    LoadModels(Which),
    /// A model picked from the listed ones.
    PickModel(Which, String),
    /// "Use a different model" for recommendations: off empties it.
    Separate(bool),
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
    /// Submit that key's field: opens the password pop-up.
    SaveKey(Which),
    /// Submit the password pop-up.
    SubmitPassword,
    CancelPassword,
    /// Open the unlock prompt.
    Unlock,
    /// Open the replace-key form.
    ShowReplace(Which),
    /// First click on Remove key: ask for confirmation.
    AskRemove(Which),
    ConfirmRemove,
    /// Close whichever key form or confirmation is open.
    CancelKey,
    AppIcon(AppIcon),
    /// A colour swatch: the accent colour, applied on Save.
    Accent(Accent),
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
    /// Close the overlay (the close button).
    Close,
    /// Fetch the endpoint's model list using these endpoint fields and the
    /// current key (for `Which::Decision`, the key only if it is on the main
    /// endpoint's host). The result arrives as `SettingsData::models`.
    ListModels(LlmSettings, Which),
    /// Save these settings (endpoint, schedule, capacity, prompts,
    /// appearance).
    Save(Settings),
    /// Send a test request using these endpoint fields and the current key
    /// (same rule as `ListModels`).
    Test(LlmSettings, Which),
    /// Encrypt and store a new API key (replacing any existing one): the
    /// main key, or the decision model's own. If the other key exists,
    /// `password` must unlock it too: one password for both.
    SetKey {
        api_key: SecretString,
        password: SecretString,
        which: Which,
    },
    RemoveKey(Which),
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
    /// The decision model's own key (`vault::decision_key_path`). It locks
    /// and unlocks together with the main key.
    pub decision_key_state: KeyState,
    /// Result of the last Test: `Ok(model reply)` or `Err(message)`.
    pub test_result: Option<Result<String, String>>,
    pub testing: bool,
    /// Why the last key operation (saving or removing a key) failed, such
    /// as a wrong password; shown near the key controls.
    pub key_message: Option<String>,
    /// Result of the last model-list fetch: `Ok(models)` (may be empty when
    /// the endpoint can't say; fall back to free text) or `Err(message)`.
    /// Only Ollama says what kind each model is; others list `Chat`.
    pub models: Option<Result<Vec<OllamaModel>, String>>,
    /// A model-list fetch is in flight.
    pub listing_models: bool,
    /// The Ollama on this machine: its models, or why it didn't answer;
    /// `None` until checked.
    pub ollama: Option<Result<Vec<OllamaModel>, String>>,
}

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

impl fmt::Display for Day {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(weekday_name(self.0))
    }
}

/// What the model-list area shows.
#[derive(Debug, PartialEq, Eq)]
enum ModelsState<'a> {
    Hidden,
    Busy,
    /// The endpoint answered but listed no models.
    Empty,
    Error(&'a str),
    /// The models this block can use (no decision models for Model).
    List(Vec<OllamaModel>),
}

/// A listed model in the Choose list: its name, tagged with what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelChoice(OllamaModel);

impl fmt::Display for ModelChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.kind {
            ModelKind::Chat => f.write_str(&self.0.name),
            ModelKind::Thinking => write!(f, "{} · thinking", self.0.name),
            ModelKind::Decision => write!(f, "{} · decision", self.0.name),
        }
    }
}

/// `url` is the Ollama that `SettingsData::ollama` checked
/// (`ollama::local_base_url`: the default port, or `OLLAMA_HOST`).
fn is_local_ollama(url: &str) -> bool {
    let root = |u: &str| ollama::root_url(u).replace("://localhost", "://127.0.0.1");
    root(url) == root(&ollama::local_base_url())
}

/// What the Provider picker offers. Ollama is the OpenAI-compatible
/// protocol with no API key (`LlmSettings::keyless`); it suits LM Studio
/// and other keyless servers too.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Provider {
    Ollama,
    #[default]
    OpenAi,
    Anthropic,
}

impl Provider {
    const ALL: [Provider; 3] = [Provider::Ollama, Provider::OpenAi, Provider::Anthropic];

    /// Ollama's is the one on this machine (`OLLAMA_HOST` if set).
    fn default_url(self) -> String {
        match self {
            Provider::Ollama => ollama::local_base_url(),
            Provider::OpenAi => "https://api.openai.com/v1".into(),
            Provider::Anthropic => "https://api.anthropic.com".into(),
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Provider::Ollama => "Ollama",
            Provider::OpenAi => "OpenAI-compatible",
            Provider::Anthropic => "Anthropic",
        })
    }
}

/// One endpoint's fields as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Endpoint {
    provider: Provider,
    base_url: String,
    model: String,
    /// An Ollama decision model (`Protocol::OllamaDecision`), only ever in
    /// the Recommendations block.
    decision: bool,
}

impl Endpoint {
    /// Keyless endpoints, and older settings on Ollama's port, show as
    /// Ollama.
    fn from_llm(llm: &LlmSettings) -> Self {
        let provider = match llm.protocol {
            Protocol::Anthropic => Provider::Anthropic,
            Protocol::OllamaDecision => Provider::Ollama,
            Protocol::OpenAi if llm.keyless || llm.base_url.contains(":11434") => Provider::Ollama,
            Protocol::OpenAi => Provider::OpenAi,
        };
        Endpoint {
            provider,
            base_url: llm.base_url.clone(),
            model: llm.model.clone(),
            decision: llm.protocol == Protocol::OllamaDecision,
        }
    }

    fn llm(&self) -> LlmSettings {
        LlmSettings {
            protocol: match self.provider {
                Provider::Anthropic => Protocol::Anthropic,
                Provider::Ollama if self.decision => Protocol::OllamaDecision,
                Provider::Ollama | Provider::OpenAi => Protocol::OpenAi,
            },
            base_url: self.base_url.trim().to_string(),
            model: self.model.trim().to_string(),
            keyless: self.provider == Provider::Ollama,
        }
    }

    /// Switches provider; an empty URL or another provider's default URL
    /// (Ollama's built-in default counts too) becomes the new provider's
    /// default.
    fn set_provider(&mut self, provider: Provider) {
        let url = self.base_url.trim();
        if url.is_empty()
            || url == OLLAMA_BASE_URL
            || Provider::ALL.iter().any(|p| p.default_url() == url)
        {
            self.base_url = provider.default_url();
        }
        if provider != Provider::Ollama {
            self.decision = false;
        }
        self.provider = provider;
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
    app_icon: AppIcon,
    accent: Accent,
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
            working_days: WEEKDAYS.map(|d| sched.working_days.contains(&d)),
            focus_hours: format_hours(s.focus_minutes_per_day),
            app_icon: s.app_icon,
            accent: s.accent,
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
    /// hours are invalid or no working day is ticked.
    fn to_settings(&self) -> Option<Settings> {
        if !self.working_days.contains(&true) {
            return None;
        }
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
                working_days: WEEKDAYS
                    .iter()
                    .zip(self.working_days)
                    .filter(|(_, on)| *on)
                    .map(|(d, _)| *d)
                    .collect(),
            },
            summary_instructions,
            focus_minutes_per_day: parse_hours(&self.focus_hours)?,
            app_icon: self.app_icon,
            accent: self.accent,
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

/// The form against the loaded settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormStatus {
    /// Nothing to save.
    Clean,
    /// Edited, but a field is invalid.
    Invalid,
    /// Edited and valid: Save saves them.
    Savable,
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
    decision_key: String,
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
            SecretField::DecisionKey => &mut self.decision_key,
            SecretField::Password => &mut self.password,
        }
    }

    fn wipe_all(&mut self) {
        wipe(&mut self.api_key);
        wipe(&mut self.decision_key);
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
    /// The settings last given to `load` (the defaults before the first
    /// one); the form resets when they change.
    loaded: Settings,
    /// The form as `loaded` fills it in, to tell which fields were edited.
    baseline: Form,
    form: Form,
    data_dir: PathBuf,
    key_state: KeyState,
    test_result: Option<Result<String, String>>,
    testing: bool,
    key_message: Option<String>,
    secrets: Secrets,
    key_mode: KeyMode,
    decision_key_state: KeyState,
    /// The key that `key_mode`, the password pop-up and `key_message`
    /// belong to.
    key_target: Which,
    /// Local validation error for the key forms.
    key_error: Option<String>,
    models: Option<Result<Vec<OllamaModel>, String>>,
    listing_models: bool,
    /// The endpoint of the last "Load models" request, to hide a list that
    /// belongs to an endpoint the form no longer names.
    listed_for: Option<(Which, Provider, String)>,
    /// "Use a different model" for recommendations is ticked.
    decision_on: bool,
    ollama: Option<Result<Vec<OllamaModel>, String>>,
    /// The first-run Ollama prefill was done (or wasn't wanted).
    ollama_prefilled: bool,
    /// The section whose Test button was pressed last; its result shows there.
    tested: Which,
    /// The endpoint that test was for: the result shows only while the
    /// form still names it.
    tested_llm: Option<LlmSettings>,
    /// The password pop-up is open (after Save key).
    password_prompt: bool,
    summary_editor: text_editor::Content,
    plan_editor: text_editor::Content,
}

impl Default for SettingsView {
    fn default() -> Self {
        let loaded = Settings::default();
        let form = Form::from_settings(&loaded);
        SettingsView {
            summary_editor: text_editor::Content::with_text(&form.summary_prompt),
            plan_editor: text_editor::Content::with_text(&form.plan_prompt),
            loaded,
            baseline: form.clone(),
            form,
            data_dir: PathBuf::new(),
            key_state: KeyState::NoKey,
            test_result: None,
            testing: false,
            key_message: None,
            secrets: Secrets::default(),
            key_mode: KeyMode::Idle,
            decision_key_state: KeyState::NoKey,
            key_target: Which::Main,
            key_error: None,
            models: None,
            listing_models: false,
            listed_for: None,
            decision_on: false,
            ollama: None,
            ollama_prefilled: false,
            tested: Which::Main,
            tested_llm: None,
            password_prompt: false,
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
        if self.loaded != data.settings {
            self.baseline = Form::from_settings(&data.settings);
            self.form = self.baseline.clone();
            self.decision_on = self.form.decision != Endpoint::default();
            self.summary_editor = text_editor::Content::with_text(&self.form.summary_prompt);
            self.plan_editor = text_editor::Content::with_text(&self.form.plan_prompt);
            self.loaded = data.settings;
        }
        if data.key_state != self.key_state || data.decision_key_state != self.decision_key_state {
            self.key_state = data.key_state;
            self.decision_key_state = data.decision_key_state;
            // Closes the password pop-up too, so nothing typed survives.
            self.close_key_form();
        }
        self.data_dir = data.data_dir;
        self.test_result = data.test_result;
        self.testing = data.testing;
        self.key_message = data.key_message;
        self.models = data.models;
        self.listing_models = data.listing_models;
        self.ollama = data.ollama;
        self.prefill_ollama();
    }

    /// First run: no model set up and Ollama is running, so the Model block
    /// starts on Ollama with its first chat model (saved only on Save).
    fn prefill_ollama(&mut self) {
        let Some(Ok(models)) = &self.ollama else {
            return;
        };
        if self.ollama_prefilled {
            return;
        }
        self.ollama_prefilled = true;
        let unset = !self.loaded.llm.is_configured();
        if unset && self.form.main == Endpoint::default() {
            let first = models.iter().find(|m| m.kind != ModelKind::Decision);
            self.form.main.set_provider(Provider::Ollama);
            self.form.main.base_url = ollama::local_base_url();
            self.form.main.model = first.map(|m| m.name.clone()).unwrap_or_default();
        }
    }

    /// What `name` is, if this block's model list knows it.
    fn kind_of_model(&self, which: Which, name: &str) -> Option<ModelKind> {
        match self.models_state(which) {
            ModelsState::List(models) => models
                .iter()
                .find(|m| m.name == name.trim())
                .map(|m| m.kind),
            _ => None,
        }
    }

    /// The edits: `None` when there is nothing to save, `Some(None)` when
    /// a field is invalid, else the settings Save would save.
    fn edits(&self) -> Option<Option<Settings>> {
        // An untouched form saves nothing even when it doesn't round-trip
        // exactly (older settings: keyless, untrimmed prompts).
        if self.form == self.baseline {
            return None;
        }
        match self.form.to_settings() {
            // Edits that parse back to the same settings ("8" → "8.0").
            Some(s) if s == self.loaded => None,
            parsed => Some(parsed),
        }
    }

    fn status(&self) -> FormStatus {
        match self.edits() {
            None => FormStatus::Clean,
            Some(None) => FormStatus::Invalid,
            Some(Some(_)) => FormStatus::Savable,
        }
    }

    /// The settings the form would save, if they differ from the loaded
    /// ones and every field is valid.
    fn savable(&self) -> Option<Settings> {
        self.edits().flatten()
    }

    #[cfg(test)]
    fn is_dirty(&self) -> bool {
        !matches!(self.status(), FormStatus::Clean)
    }

    fn can_test(&self, which: Which) -> bool {
        self.form.endpoint(which).llm().is_configured() && !self.testing
    }

    fn can_list_models(&self, which: Which) -> bool {
        !self.form.endpoint(which).base_url.trim().is_empty() && !self.listing_models
    }

    /// The models this block can pick from: the last Load for it, or the
    /// local Ollama check when the block is on Ollama's default address. A
    /// list for the other block or another endpoint isn't shown.
    fn models_state(&self, which: Which) -> ModelsState<'_> {
        let e = self.form.endpoint(which);
        let usable = |models: &[OllamaModel]| -> Vec<OllamaModel> {
            models
                .iter()
                .filter(|m| which == Which::Decision || m.kind != ModelKind::Decision)
                .cloned()
                .collect()
        };
        let listed_here = self.listed_for.as_ref().is_some_and(|(w, provider, url)| {
            *w == which && *provider == e.provider && *url == e.base_url.trim()
        });
        if listed_here {
            if self.listing_models {
                return ModelsState::Busy;
            }
            match &self.models {
                Some(Ok(models)) => {
                    let list = usable(models);
                    return if list.is_empty() {
                        ModelsState::Empty
                    } else {
                        ModelsState::List(list)
                    };
                }
                Some(Err(e)) => return ModelsState::Error(e),
                None => {}
            }
        }
        if e.provider == Provider::Ollama
            && is_local_ollama(&e.base_url)
            && let Some(Ok(models)) = &self.ollama
        {
            let list = usable(models);
            if !list.is_empty() {
                return ModelsState::List(list);
            }
        }
        ModelsState::Hidden
    }

    pub fn update(&mut self, message: Message) -> Action {
        let form = &mut self.form;
        match message {
            Message::Provider(w, p) => {
                form.endpoint_mut(w).set_provider(p);
                if p == Provider::Ollama {
                    return self.update(Message::LoadModels(w));
                }
            }
            Message::Separate(on) => {
                self.decision_on = on;
                if !on {
                    form.decision = Endpoint::default();
                } else if form.decision == Endpoint::default() {
                    return self.update(Message::Provider(Which::Decision, Provider::Ollama));
                }
            }
            Message::BaseUrl(w, s) => form.endpoint_mut(w).base_url = s,
            Message::Model(w, s) | Message::PickModel(w, s) => {
                // Only a model the list calls a decision model counts as one.
                let decision =
                    w == Which::Decision && self.kind_of_model(w, &s) == Some(ModelKind::Decision);
                let e = self.form.endpoint_mut(w);
                e.model = s;
                e.decision = decision;
            }
            Message::FocusHours(s) => form.focus_hours = s,
            Message::LoadModels(w) => {
                if self.can_list_models(w) {
                    let llm = self.form.endpoint(w).llm();
                    let provider = self.form.endpoint(w).provider;
                    self.listed_for = Some((w, provider, llm.base_url.clone()));
                    return Action::ListModels(llm, w);
                }
            }
            Message::Close => {
                self.on_hidden();
                return Action::Close;
            }
            Message::WeekStart(d) => form.week_start = d,
            Message::AppIcon(choice) => form.app_icon = choice,
            Message::Accent(accent) => form.accent = accent,
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
                let edit = action.is_edit();
                editor.perform(action);
                if edit {
                    let text = editor.text();
                    match kind {
                        PromptKind::Summary => form.summary_prompt = text,
                        PromptKind::Plan => form.plan_prompt = text,
                    }
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
                    let llm = self.form.endpoint(w).llm();
                    self.tested = w;
                    self.tested_llm = Some(llm.clone());
                    return Action::Test(llm, w);
                }
            }
            Message::Secret(field, Typed(value)) => {
                let slot = self.secrets.field(field);
                wipe(slot);
                *slot = value;
            }
            Message::SaveKey(which) => {
                if self.key_target != which {
                    // The other key's form or confirmation closes.
                    self.key_mode = KeyMode::Idle;
                    self.key_target = which;
                }
                if self.typed_key(which).trim().is_empty() {
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
            Message::ShowReplace(which) => self.open_key_form(which, KeyMode::Replace),
            Message::AskRemove(which) => self.open_key_form(which, KeyMode::ConfirmRemove),
            Message::ConfirmRemove => {
                if self.key_mode == KeyMode::ConfirmRemove {
                    let which = self.key_target;
                    self.close_key_form();
                    return Action::RemoveKey(which);
                }
            }
            Message::CancelKey => self.close_key_form(),
        }
        Action::None
    }

    /// The overlay was hidden, by any route: typed but unsubmitted secrets
    /// don't outlive it, and the key form and password pop-up close. Form
    /// edits are kept.
    pub fn on_hidden(&mut self) {
        self.close_key_form();
    }

    fn open_key_form(&mut self, which: Which, mode: KeyMode) {
        self.close_key_form();
        self.key_target = which;
        self.key_mode = mode;
    }

    /// The key typed into `which`'s field.
    fn typed_key(&self, which: Which) -> &str {
        match which {
            Which::Main => &self.secrets.api_key,
            Which::Decision => &self.secrets.decision_key,
        }
    }

    /// The state of `which`'s saved key.
    fn key_state_of(&self, which: Which) -> KeyState {
        match which {
            Which::Main => self.key_state,
            Which::Decision => self.decision_key_state,
        }
    }

    fn close_key_form(&mut self) {
        self.key_mode = KeyMode::Idle;
        self.key_error = None;
        self.password_prompt = false;
        self.secrets.wipe_all();
    }

    fn save_key(&mut self) -> Action {
        let which = self.key_target;
        if self.typed_key(which).trim().is_empty() {
            self.password_prompt = false;
            wipe(&mut self.secrets.password);
            return Action::None;
        }
        if let Err(e) = check_password(&self.secrets.password) {
            self.key_error = Some(e);
            return Action::None;
        }
        let api_key = SecretString::from(self.typed_key(which).trim().to_string());
        let password = take_secret(&mut self.secrets.password);
        self.close_key_form();
        Action::SetKey {
            api_key,
            password,
            which,
        }
    }

    // ------------------------------------------------------------- view

    pub fn view(&self) -> Element<'_, Message> {
        let status = self.status();
        let save = button(text("Save").size(theme::TEXT_BODY))
            .padding([8, 18])
            .style(theme::primary_button)
            .on_press_maybe((status == FormStatus::Savable).then_some(Message::Save));
        let close = button(theme::icon(Icon::Close))
            .padding(theme::ICON_PADDING)
            .style(theme::icon_button)
            .on_press(Message::Close);
        let status: Element<'_, Message> = match status {
            FormStatus::Clean => Space::new().into(),
            FormStatus::Invalid => text("Fix the errors below to save")
                .size(theme::TEXT_SMALL)
                .style(theme::danger)
                .into(),
            FormStatus::Savable => text("Unsaved changes")
                .size(theme::TEXT_SMALL)
                .style(theme::muted)
                .into(),
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
            self.model_section(),
            self.recommendations_section(),
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
        let other = match self.key_target {
            Which::Main => Which::Decision,
            Which::Decision => Which::Main,
        };
        let title = if self.key_state_of(other) == KeyState::NoKey {
            "Set a password"
        } else {
            "Your key password"
        };
        let mut card =
            column![text(title).size(theme::TEXT_HEADING), field].spacing(theme::SPACING);
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

    fn model_section(&self) -> Element<'_, Message> {
        section("Model", self.endpoint_grid(Which::Main))
    }

    fn recommendations_section(&self) -> Element<'_, Message> {
        let toggle = checkbox(self.decision_on)
            .label("Use a different model")
            .text_size(theme::TEXT_BODY)
            .on_toggle(Message::Separate);
        let mut body = column![toggle].spacing(theme::SPACING);
        if self.decision_on {
            body = body.push(self.endpoint_grid(Which::Decision));
        }
        section("Recommendations", body)
    }

    /// Provider, URL, model, API key (when the provider takes one) and Test
    /// for one endpoint, as label/field rows.
    fn endpoint_grid(&self, which: Which) -> Element<'_, Message> {
        let f = self.form.endpoint(which);
        let picker_list = pick_list(Provider::ALL, Some(f.provider), move |p| {
            Message::Provider(which, p)
        })
        .padding([8, 12])
        .text_size(theme::TEXT_BODY)
        .width(240)
        .style(picker);
        let mut provider = row![picker_list].spacing(theme::SPACING).align_y(Center);
        if f.provider == Provider::Ollama && is_local_ollama(&f.base_url) {
            provider = provider.push(match &self.ollama {
                Some(Ok(_)) => text("Running").size(theme::TEXT_SMALL).style(success),
                Some(Err(_)) => text("Not running")
                    .size(theme::TEXT_SMALL)
                    .style(theme::muted),
                None => text("Checking…")
                    .size(theme::TEXT_SMALL)
                    .style(theme::muted),
            });
        }
        let url = input(&f.provider.default_url(), &f.base_url)
            .on_input(move |s| Message::BaseUrl(which, s));

        let model_input = input("Model name", &f.model).on_input(move |s| Message::Model(which, s));
        let state = self.models_state(which);
        let error = match &state {
            ModelsState::Error(e) => Some(shorten(e, 120)),
            _ => None,
        };
        let pick: Element<'_, Message> = match state {
            ModelsState::List(models) => {
                let choices: Vec<ModelChoice> = models.into_iter().map(ModelChoice).collect();
                let selected = choices.iter().find(|c| c.0.name == f.model.trim()).cloned();
                pick_list(choices, selected, move |c: ModelChoice| {
                    Message::PickModel(which, c.0.name)
                })
                .placeholder("Choose")
                .padding([8, 12])
                .text_size(theme::TEXT_BODY)
                .width(200)
                .style(picker)
                .into()
            }
            ModelsState::Busy => button(text("Loading…").size(theme::TEXT_BODY))
                .padding([8, 14])
                .style(theme::secondary_button)
                .into(),
            _ => button(text("Load").size(theme::TEXT_BODY))
                .padding([8, 14])
                .style(theme::secondary_button)
                .on_press_maybe(
                    self.can_list_models(which)
                        .then_some(Message::LoadModels(which)),
                )
                .into(),
        };
        let mut model = column![
            row![model_input, pick]
                .spacing(theme::SPACING / 2.0)
                .align_y(Center)
        ]
        .spacing(theme::SPACING / 3.0);
        if let Some(e) = error {
            model = model.push(error_text(e));
        }

        let testing_here = self.testing && self.tested == which;
        let test =
            button(text(if testing_here { "Testing…" } else { "Test" }).size(theme::TEXT_BODY))
                .padding([8, 14])
                .style(theme::secondary_button)
                .on_press_maybe(self.can_test(which).then_some(Message::Test(which)));
        let mut test_row = row![test].spacing(theme::SPACING).align_y(Center);
        if self.tested == which && !self.testing && self.tested_llm.as_ref() == Some(&f.llm()) {
            match &self.test_result {
                Some(Ok(reply)) => {
                    test_row = test_row.push(
                        text(shorten(reply, 80))
                            .size(theme::TEXT_SMALL)
                            .style(success)
                            .width(Fill),
                    )
                }
                Some(Err(e)) => test_row = test_row.push(error_text(shorten(e, 160))),
                None => {}
            }
        }

        let mut grid = column![
            field_row("Provider", provider),
            field_row("URL", url),
            field_row("Model", model),
        ]
        .spacing(theme::SPACING);
        if f.provider != Provider::Ollama {
            grid = grid.push(field_row("API key", self.key_controls(which)));
        }
        grid.push(field_row("", test_row)).into()
    }

    /// One key: its field and Save, or its state with Unlock, Replace and
    /// Remove. Errors about it show underneath.
    fn key_controls(&self, which: Which) -> Element<'_, Message> {
        let here = self.key_target == which;
        let mode = if here { self.key_mode } else { KeyMode::Idle };
        let status = |label: &'static str| {
            text(label)
                .size(theme::TEXT_BODY)
                .style(theme::muted)
                .width(Fill)
        };
        let line: Element<'_, Message> = match (self.key_state_of(which), mode) {
            (_, KeyMode::ConfirmRemove) => row![
                status("Remove this key?"),
                button(text("Remove").size(theme::TEXT_BODY))
                    .padding([8, 14])
                    .style(theme::danger_button)
                    .on_press(Message::ConfirmRemove),
                cancel_button(),
            ]
            .spacing(theme::SPACING / 2.0)
            .align_y(Center)
            .into(),
            (KeyState::NoKey, _) => self.key_input(which, false),
            (_, KeyMode::Replace) => self.key_input(which, true),
            (KeyState::Locked, KeyMode::Idle) => row![
                status("Locked"),
                secondary("Unlock", Message::Unlock),
                secondary("Replace", Message::ShowReplace(which)),
                remove_button(which),
            ]
            .spacing(theme::SPACING / 2.0)
            .align_y(Center)
            .into(),
            (KeyState::Unlocked, KeyMode::Idle) => row![
                status("Saved"),
                secondary("Replace", Message::ShowReplace(which)),
                remove_button(which),
            ]
            .spacing(theme::SPACING / 2.0)
            .align_y(Center)
            .into(),
        };
        let error = if !here || self.password_prompt {
            None
        } else {
            self.key_error.clone().or_else(|| self.key_message.clone())
        };
        match error {
            Some(e) => column![line, error_text(e)]
                .spacing(theme::SPACING / 3.0)
                .into(),
            None => line,
        }
    }

    /// A key field and Save (Enter works too).
    fn key_input(&self, which: Which, cancellable: bool) -> Element<'_, Message> {
        let field = match which {
            Which::Main => SecretField::ApiKey,
            Which::Decision => SecretField::DecisionKey,
        };
        let mut line = row![
            secret_input("Paste API key", self.typed_key(which), field)
                .on_submit(Message::SaveKey(which)),
            secondary("Save", Message::SaveKey(which)),
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
        let ticked = f.working_days.iter().filter(|on| **on).count();
        for (i, day) in WEEKDAYS.iter().enumerate() {
            // The last ticked day can't be unticked.
            let last = f.working_days[i] && ticked == 1;
            days = days.push(
                checkbox(f.working_days[i])
                    .label(short_weekday(*day))
                    .text_size(theme::TEXT_BODY)
                    .on_toggle_maybe((!last).then_some(move |on| Message::WorkingDay(i, on))),
            );
        }
        let mut hours = column![
            text("Work hours per day").size(theme::TEXT_SMALL),
            input("8", &f.focus_hours)
                .on_input(Message::FocusHours)
                .width(90),
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
        let swatches = row(Accent::ALL
            .into_iter()
            .map(|a| swatch(a, self.form.accent == a)))
        .spacing(theme::SPACING / 2.0)
        .align_y(Center);
        section(
            "Appearance",
            column![labelled("Color", swatches), labelled("App icon", icon)]
                .spacing(theme::SPACING),
        )
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

/// A round colour swatch for an accent choice: a ring around it when it
/// is the one picked (a faint one on hover), its name on hover.
fn swatch<'a>(accent: Accent, picked: bool) -> Element<'a, Message> {
    let color = theme::choice_color(accent);
    let dot =
        container(Space::new().width(22).height(22)).style(move |_: &Theme| container::Style {
            background: Some(color.into()),
            border: Border {
                radius: 999.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        });
    let ring = move |_: &Theme, status: button::Status| {
        let ring = match (picked, status) {
            (true, _) => color,
            (false, button::Status::Hovered | button::Status::Pressed) => color.scale_alpha(0.45),
            _ => Color::TRANSPARENT,
        };
        button::Style {
            border: Border {
                radius: 999.0.into(),
                width: 2.0,
                color: ring,
            },
            ..button::Style::default()
        }
    };
    tooltip(
        button(dot)
            .padding(3)
            .style(ring)
            .on_press(Message::Accent(accent)),
        theme::tip(accent.label()),
        tooltip::Position::Bottom,
    )
    .into()
}

fn section<'a>(heading: &'a str, body: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    column![theme::section_heading(heading), body.into()]
        .spacing(theme::SPACING)
        .into()
}

/// A fixed-width label and its field, for the model settings.
fn field_row<'a>(label: &'a str, field: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    row![
        text(label).size(theme::TEXT_BODY).width(90),
        container(field).width(Fill)
    ]
    .spacing(theme::SPACING)
    .align_y(Center)
    .into()
}

fn error_text<'a>(message: String) -> Element<'a, Message> {
    text(message)
        .size(theme::TEXT_SMALL)
        .style(theme::danger)
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
    pick_list(WEEKDAYS.map(Day), Some(Day(day)), move |d: Day| {
        on_select(d.0)
    })
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

fn remove_button<'a>(which: Which) -> Element<'a, Message> {
    button(text("Remove key").size(theme::TEXT_BODY))
        .padding([8, 14])
        .style(theme::danger_button)
        .on_press(Message::AskRemove(which))
        .into()
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
    let ring = match status {
        pick_list::Status::Opened { .. } => theme::accent_color(),
        pick_list::Status::Hovered => theme::tint(theme, 0.15),
        pick_list::Status::Active => Color::TRANSPARENT,
    };
    pick_list::Style {
        text_color: p.text,
        placeholder_color: theme::muted_color(theme),
        handle_color: theme::muted_color(theme),
        background: Background::Color(theme::tint(theme, 0.075)),
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

    /// Wipes the typed password and clears the error, for the next time
    /// the prompt opens. `main.rs` calls it whenever the prompt closes.
    pub fn reset(&mut self) {
        wipe(&mut self.password);
        self.error = None;
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
                self.reset();
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
    use jiff::civil::Time;
    use secrecy::ExposeSecret;

    fn data(settings: Settings, key_state: KeyState) -> SettingsData {
        SettingsData {
            settings,
            data_dir: PathBuf::from("/tmp/fortnight-data"),
            key_state,
            decision_key_state: KeyState::NoKey,
            test_result: None,
            testing: false,
            key_message: None,
            models: None,
            listing_models: false,
            ollama: None,
        }
    }

    fn chat(name: &str) -> OllamaModel {
        OllamaModel {
            name: name.into(),
            kind: ModelKind::Chat,
        }
    }

    fn model(name: &str, kind: ModelKind) -> OllamaModel {
        OllamaModel {
            name: name.into(),
            kind,
        }
    }

    #[test]
    fn local_ollama_lists_models_by_kind_and_decision_only_for_recommendations() {
        let mut v = loaded(KeyState::NoKey);
        let found = vec![
            model("deepseek-r1:8b", ModelKind::Thinking),
            model("llama3.2", ModelKind::Chat),
            model("nimble", ModelKind::Decision),
        ];
        let mut d = data(Settings::default(), KeyState::NoKey);
        d.ollama = Some(Ok(found.clone()));
        v.load(d.clone());
        // First run: Model starts on Ollama with its first chat model.
        assert_eq!(v.form.main.provider, Provider::Ollama);
        assert_eq!(v.form.main.model, "deepseek-r1:8b");
        assert_eq!(
            v.models_state(Which::Main),
            ModelsState::List(found[..2].to_vec())
        );
        let Action::Save(saved) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert!(saved.llm.keyless);
        assert_eq!(saved.llm.protocol, Protocol::OpenAi);

        let mut d2 = d.clone();
        d2.settings = saved;
        v.load(d2);
        v.update(Message::Separate(true));
        // Recommendations lists the local models straight away, the
        // decision model too.
        assert_eq!(
            v.models_state(Which::Decision),
            ModelsState::List(found.clone())
        );
        v.update(Message::PickModel(Which::Decision, "nimble".into()));
        let Action::Save(saved) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(saved.decision_llm.protocol, Protocol::OllamaDecision);
        assert_eq!(saved.decision_llm.model, "nimble");
        // It loads back as Ollama with the decision model.
        v.load(data(saved.clone(), KeyState::NoKey));
        assert_eq!(v.form.decision.provider, Provider::Ollama);
        assert!(v.form.decision.decision);
        assert!(!v.is_dirty());
        // Typing a name it doesn't know as a decision model makes it a chat
        // model again.
        v.update(Message::Model(Which::Decision, "qwen3".into()));
        assert!(!v.form.decision.llm().protocol.eq(&Protocol::OllamaDecision));
    }

    #[test]
    fn ollama_prefill_leaves_a_configured_model_alone() {
        let mut configured = Settings::default();
        configured.llm.base_url = "https://api.openai.com/v1".into();
        configured.llm.model = "gpt-4o".into();
        let mut d = data(configured, KeyState::NoKey);
        d.ollama = Some(Ok(vec![chat("llama3.2")]));
        let mut v = SettingsView::new();
        v.load(d);
        assert_eq!(v.form.main.provider, Provider::OpenAi);
        assert_eq!(v.form.main.model, "gpt-4o");
        assert!(!v.is_dirty());
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
    fn accent_choice_saves() {
        let mut v = loaded(KeyState::NoKey);
        assert_eq!(v.form.accent, Accent::Orange);
        v.update(Message::Accent(Accent::Purple));
        assert!(v.is_dirty());
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.accent, Accent::Purple);
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
    fn save_key_asks_for_a_password_then_emits() {
        let mut v = loaded(KeyState::NoKey);
        assert!(matches!(
            v.update(Message::SaveKey(Which::Main)),
            Action::None
        ));
        assert!(v.key_error.is_some(), "no key typed");
        assert!(!v.password_prompt);

        typed(&mut v, SecretField::ApiKey, " sk-abc ");
        assert!(matches!(
            v.update(Message::SaveKey(Which::Main)),
            Action::FocusPassword
        ));
        assert!(v.password_prompt);
        typed(&mut v, SecretField::Password, "short");
        assert!(matches!(v.update(Message::SubmitPassword), Action::None));
        assert!(v.key_error.is_some());
        assert!(v.password_prompt, "stays open to fix it");

        typed(&mut v, SecretField::Password, "correct horse");
        let Action::SetKey {
            api_key, password, ..
        } = v.update(Message::SubmitPassword)
        else {
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
        v.update(Message::SaveKey(Which::Main));
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
    fn provider_fills_its_url_and_ollama_lists_models() {
        let mut v = loaded(KeyState::Locked);
        v.update(Message::Provider(Which::Main, Provider::Anthropic));
        assert_eq!(v.form.main.base_url, "https://api.anthropic.com");
        let Action::ListModels(llm, Which::Main) =
            v.update(Message::Provider(Which::Main, Provider::Ollama))
        else {
            panic!("expected ListModels");
        };
        assert_eq!(llm.protocol, Protocol::OpenAi);
        assert!(llm.keyless);
        assert_eq!(llm.base_url, ollama::local_base_url());
        // A typed URL is kept.
        v.update(Message::BaseUrl(
            Which::Main,
            "http://192.168.1.20:11434/v1".into(),
        ));
        v.update(Message::Provider(Which::Main, Provider::OpenAi));
        assert_eq!(v.form.main.base_url, "http://192.168.1.20:11434/v1");
        assert!(!v.form.main.llm().keyless);
    }

    #[test]
    fn recommendations_model_saves_tests_and_turns_off() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::Provider(Which::Main, Provider::OpenAi));
        v.update(Message::Model(Which::Main, "gpt-4o".into()));
        let Action::ListModels(llm, Which::Decision) = v.update(Message::Separate(true)) else {
            panic!("expected ListModels for the recommendations model");
        };
        assert!(v.decision_on);
        assert_eq!(llm.base_url, ollama::local_base_url());
        // The list belongs to the recommendations block only.
        assert_eq!(v.models_state(Which::Main), ModelsState::Hidden);
        v.update(Message::PickModel(Which::Decision, "deepseek-r1:8b".into()));
        let Action::Test(llm, Which::Decision) = v.update(Message::Test(Which::Decision)) else {
            panic!("expected Test for the recommendations model");
        };
        assert_eq!(llm.model, "deepseek-r1:8b");

        let Action::Save(saved) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(saved.llm.model, "gpt-4o");
        assert_eq!(saved.decision_llm.base_url, ollama::local_base_url());
        assert_eq!(saved.decision_llm.model, "deepseek-r1:8b");
        assert!(saved.decision_llm.keyless);

        v.load(data(saved, KeyState::NoKey));
        assert!(v.decision_on);
        v.update(Message::Separate(false));
        let Action::Save(cleared) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert!(!cleared.decision_llm.is_configured());
        assert_eq!(cleared.llm.model, "gpt-4o");
        v.load(data(cleared, KeyState::NoKey));
        assert!(!v.decision_on);
    }

    #[test]
    fn decision_key_has_its_own_field_and_actions() {
        let mut v = loaded(KeyState::Unlocked);
        typed(&mut v, SecretField::ApiKey, "sk-main-typed");
        typed(&mut v, SecretField::DecisionKey, " ds-key ");
        assert!(matches!(
            v.update(Message::SaveKey(Which::Decision)),
            Action::FocusPassword
        ));
        typed(&mut v, SecretField::Password, "correct horse");
        let Action::SetKey {
            api_key,
            password,
            which: Which::Decision,
        } = v.update(Message::SubmitPassword)
        else {
            panic!("expected SetKey for the decision key");
        };
        assert_eq!(api_key.expose_secret(), "ds-key");
        assert_eq!(password.expose_secret(), "correct horse");

        let mut d = data(Settings::default(), KeyState::Unlocked);
        d.decision_key_state = KeyState::Unlocked;
        v.load(d);
        v.update(Message::AskRemove(Which::Decision));
        assert_eq!(v.key_target, Which::Decision);
        assert!(matches!(
            v.update(Message::ConfirmRemove),
            Action::RemoveKey(Which::Decision)
        ));
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
        v.update(Message::AskRemove(Which::Main));
        assert_eq!(v.key_mode, KeyMode::ConfirmRemove);
        v.update(Message::CancelKey);
        assert_eq!(v.key_mode, KeyMode::Idle);
        v.update(Message::AskRemove(Which::Main));
        assert!(matches!(
            v.update(Message::ConfirmRemove),
            Action::RemoveKey(Which::Main)
        ));
        assert_eq!(v.key_mode, KeyMode::Idle);
    }

    #[test]
    fn settings_unlock() {
        let mut v = loaded(KeyState::Locked);
        assert!(matches!(v.update(Message::Unlock), Action::ShowUnlock));
    }

    #[test]
    fn key_state_change_closes_forms_and_wipes_secrets() {
        let mut v = loaded(KeyState::Unlocked);
        v.update(Message::ShowReplace(Which::Main));
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
        assert_eq!(v.form.focus_hours, "8");
        v.update(Message::Provider(Which::Main, Provider::Anthropic));
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
        assert_eq!(v.form.main.provider, Provider::Anthropic);
        assert_eq!(v.form.focus_hours, "5.5");
        assert!(!v.is_dirty());
        assert!(matches!(v.update(Message::Save), Action::None));
    }

    #[test]
    fn round1_settings_load_with_defaults_and_save_unchanged() {
        // Round 1 settings JSON has no protocol or focus minutes; core's
        // serde defaults fill them in (tested in `domain`), which gives this
        // value.
        let mut round1 = Settings::default();
        round1.llm.base_url = "http://localhost:11434/v1".into();
        round1.llm.model = "llama3.2".into();
        round1.summary_instructions = "Short.".into();
        let mut v = SettingsView::new();
        v.load(data(round1.clone(), KeyState::NoKey));
        // On Ollama's port, so it shows as Ollama.
        assert_eq!(v.form.main.provider, Provider::Ollama);
        assert_eq!(v.form.focus_hours, "8");
        assert!(!v.is_dirty());
        v.update(Message::FocusHours("8.5".into()));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        // Saved as Ollama: the same endpoint, now marked keyless.
        assert_eq!(s.llm.base_url, round1.llm.base_url);
        assert_eq!(s.llm.model, round1.llm.model);
        assert_eq!(s.llm.protocol, round1.llm.protocol);
        assert!(s.llm.keyless);
        assert_eq!(s.schedule, round1.schedule);
        assert_eq!(s.focus_minutes_per_day, DEFAULT_FOCUS_MINUTES + 30);
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
    fn list_models_carries_current_fields() {
        let mut v = loaded(KeyState::NoKey);
        // Needs a base URL.
        assert!(matches!(
            v.update(Message::LoadModels(Which::Main)),
            Action::None
        ));
        v.update(Message::Provider(Which::Main, Provider::Anthropic));
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
        // Not a provider's default URL, so switching provider keeps it.
        v.update(Message::BaseUrl(
            Which::Main,
            "http://localhost:8080/v1".into(),
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

        let ids = vec![chat("llama3.2"), chat("qwen3")];
        v.load(with(Ok(ids.clone())));
        assert_eq!(v.models_state(Which::Main), ModelsState::List(ids.clone()));
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
        v.update(Message::Provider(Which::Main, Provider::Anthropic));
        assert_eq!(v.models_state(Which::Main), ModelsState::Hidden);
        v.update(Message::Provider(Which::Main, Provider::OpenAi));
        assert_eq!(v.models_state(Which::Main), ModelsState::List(ids));
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

    #[test]
    fn on_hidden_wipes_secrets_and_closes_key_forms_but_keeps_edits() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::Model(Which::Main, "unsaved".into()));
        typed(&mut v, SecretField::ApiKey, "sk-abc");
        v.update(Message::SaveKey(Which::Main));
        typed(&mut v, SecretField::Password, "correct horse");
        assert!(v.password_prompt);
        v.on_hidden();
        assert!(!v.password_prompt);
        assert_eq!(v.key_mode, KeyMode::Idle);
        assert!(v.key_error.is_none());
        assert!(v.secrets.api_key.is_empty());
        assert!(v.secrets.password.is_empty());
        assert_eq!(v.form.main.model, "unsaved");

        // The close button goes the same way.
        let mut v = loaded(KeyState::Unlocked);
        v.update(Message::ShowReplace(Which::Main));
        typed(&mut v, SecretField::ApiKey, "sk-abc");
        assert!(matches!(v.update(Message::Close), Action::Close));
        assert_eq!(v.key_mode, KeyMode::Idle);
        assert!(v.secrets.api_key.is_empty());
    }

    #[test]
    fn test_result_shows_only_for_the_tested_endpoint() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::BaseUrl(
            Which::Main,
            "http://localhost:8080/v1".into(),
        ));
        v.update(Message::Model(Which::Main, "llama3.2".into()));
        let Action::Test(..) = v.update(Message::Test(Which::Main)) else {
            panic!("expected Test");
        };
        let mut d = data(Settings::default(), KeyState::NoKey);
        d.test_result = Some(Err("404 Not Found".into()));
        v.load(d);
        let shown =
            |v: &SettingsView| v.tested_llm.as_ref() == Some(&v.form.endpoint(Which::Main).llm());
        assert!(shown(&v));
        // Fixing the URL hides the old result; a trimmed-equal URL doesn't.
        v.update(Message::BaseUrl(
            Which::Main,
            "http://localhost:8081/v1".into(),
        ));
        assert!(!shown(&v));
        v.update(Message::BaseUrl(
            Which::Main,
            " http://localhost:8080/v1 ".into(),
        ));
        assert!(shown(&v));
        v.update(Message::Model(Which::Main, "qwen3".into()));
        assert!(!shown(&v));
        // Rendering works either way.
        let _ = v.view();
    }

    #[test]
    fn edits_that_parse_to_the_loaded_settings_are_not_unsaved() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::FocusHours("8.0".into()));
        v.update(Message::PlanTime("8:00".into()));
        assert!(!v.is_dirty());
        assert!(v.savable().is_none());
        assert!(matches!(v.update(Message::Save), Action::None));
        v.update(Message::FocusHours("8.5".into()));
        assert!(v.is_dirty());
        assert!(v.savable().is_some());
        // Invalid counts as unsaved (the header asks to fix it).
        v.update(Message::FocusHours("x".into()));
        assert!(v.is_dirty());
        assert!(v.savable().is_none());
    }

    #[test]
    fn key_state_change_under_the_password_prompt_closes_it() {
        let mut v = loaded(KeyState::NoKey);
        typed(&mut v, SecretField::ApiKey, "sk-abc");
        v.update(Message::SaveKey(Which::Main));
        typed(&mut v, SecretField::Password, "correct horse");
        v.load(data(Settings::default(), KeyState::Unlocked));
        assert!(!v.password_prompt);
        assert!(v.secrets.api_key.is_empty());
        assert!(v.secrets.password.is_empty());

        // Submitting with the key gone wipes the password too.
        typed(&mut v, SecretField::ApiKey, "sk-abc");
        v.update(Message::SaveKey(Which::Main));
        typed(&mut v, SecretField::ApiKey, "");
        typed(&mut v, SecretField::Password, "correct horse");
        assert!(matches!(v.update(Message::SubmitPassword), Action::None));
        assert!(!v.password_prompt);
        assert!(v.secrets.password.is_empty());
    }

    #[test]
    fn ollama_defaults_follow_the_local_address() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::Provider(Which::Main, Provider::Ollama));
        assert_eq!(v.form.main.base_url, ollama::local_base_url());
        v.update(Message::Separate(true));
        assert_eq!(v.form.decision.base_url, ollama::local_base_url());
        // The built-in address still counts as a default when switching.
        v.update(Message::BaseUrl(Which::Main, OLLAMA_BASE_URL.into()));
        v.update(Message::Provider(Which::Main, Provider::Anthropic));
        assert_eq!(v.form.main.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn no_working_days_cannot_be_saved() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::Model(Which::Main, "m".into()));
        for i in 0..7 {
            v.update(Message::WorkingDay(i, false));
        }
        assert!(v.form.to_settings().is_none());
        assert!(matches!(v.update(Message::Save), Action::None));
        v.update(Message::WorkingDay(2, true));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.schedule.working_days, vec![Weekday::Wednesday]);
    }

    #[test]
    fn prompt_text_is_copied_only_on_edits() {
        let mut v = loaded(KeyState::NoKey);
        // A stale form copy shows that a move didn't copy the editor text.
        v.form.summary_prompt = "stale".into();
        v.update(Message::Prompt(
            PromptKind::Summary,
            text_editor::Action::Move(text_editor::Motion::DocumentEnd),
        ));
        assert_eq!(v.form.summary_prompt, "stale");
        v.update(Message::Prompt(
            PromptKind::Summary,
            text_editor::Action::Edit(text_editor::Edit::Insert('!')),
        ));
        assert_eq!(v.form.summary_prompt, format!("{DEFAULT_SUMMARY_PROMPT}!"));
    }

    #[test]
    fn unlock_reset_wipes_password_and_error() {
        let mut u = UnlockView::new();
        u.update(UnlockMessage::Password(Typed("wrong".into())));
        u.set_error("wrong password".into());
        u.update(UnlockMessage::Password(Typed("typed".into())));
        u.reset();
        assert!(u.password.is_empty());
        assert!(u.error.is_none());
        // Cancel does the same.
        u.update(UnlockMessage::Password(Typed("typed".into())));
        u.set_error("wrong password".into());
        u.update(UnlockMessage::Password(Typed("again".into())));
        assert!(matches!(u.update(UnlockMessage::Skip), UnlockAction::Skip));
        assert!(u.password.is_empty());
        assert!(u.error.is_none());
    }
}
