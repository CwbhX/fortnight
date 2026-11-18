//! Settings overlay. The unlock prompt is in `views::unlock`.
//!
//! Settings opens as an overlay panel over the current tab. It is one
//! scrollable column under a fixed header that holds the title, the single
//! Save button (it saves the whole form: appearance, endpoint and protocol,
//! week, and the two prompts) and a close button. Closing with unsaved
//! changes is allowed: the header says "Unsaved changes", and the edits stay
//! in the form until the saved settings change.
//!
//! Order, top to bottom: Appearance (Theme and Color side by side, then App
//! icon), Model, Recommendations, Week (first day, working days and work
//! hours), Data folder, and Prompts last, folded shut until its heading is
//! clicked (they are long). There are no report times: the summary's
//! background update follows the working days (`week::due_reports`).
//! Theme and Color apply on Save, like everything else in the form.
//!
//! Models: two blocks of label/field rows, Model (everything) and
//! Recommendations (off unless "Use a different model" is ticked; then it
//! is `Settings::decision_llm`). Each block has Provider (Ollama, which is
//! keyless; OpenAI-compatible; Anthropic), URL, Model (a list the endpoint
//! fills; never typed, except for an endpoint that can't list its models),
//! API key (not for Ollama) and Test. No help text, by request.
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
//! Picking a provider fills in its default URL (unless one was typed).
//! Each block's model list loads by itself: when Settings opens, on picking
//! a provider, a moment after the URL is edited, and when a key is saved or
//! unlocked (`Message::LoadModels`, sent by `main.rs` for the last two). A
//! provider that needs a key loads nothing until one is saved. The saved
//! model shows even when the list doesn't have it. Ollama on this machine is checked at
//! launch and when Settings opens (`SettingsData::ollama`): an Ollama block
//! on the default address shows Running / Not running and lists its models
//! from that check, tagged "thinking" or "decision". Decision models (Ollama's
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

use fortnight_core::domain::{Accent, AppIcon, Appearance, LlmSettings, Settings};
use fortnight_core::ollama::{ModelKind, OllamaModel};
use fortnight_core::parse::parse_hours;
use fortnight_core::report::{
    DEFAULT_PLAN_PROMPT, DEFAULT_SUMMARY_PROMPT, WEEKDAYS, short_weekday, weekday_name,
};
use iced::widget::{
    Space, button, checkbox, column, container, opaque, pick_list, row, rule, scrollable, stack,
    text, text_editor, text_input, tooltip,
};
use iced::{Background, Border, Center, Color, Element, Fill, Theme};
use jiff::civil::Weekday;
use secrecy::SecretString;
use std::fmt;
use std::path::PathBuf;

use crate::theme::{self, Icon};

mod endpoint;
mod form;
mod keys;

pub use super::unlock::{UnlockAction, UnlockMessage, UnlockView};
use endpoint::Endpoint;
pub use endpoint::Provider;
use form::{Form, FormStatus};
use keys::{KeyMode, Secrets};
pub use keys::{KeyState, PASSWORD_INPUT_ID, SecretField, Typed};
pub(crate) use keys::{take_secret, wipe};

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
    /// Ask the endpoint for its model list (nothing happens if it can't
    /// be asked yet: no URL, or no key for a provider that needs one).
    LoadModels(Which),
    /// A while after a URL edit: loads that block's models if the URL is
    /// still this.
    UrlSettled(Which, String),
    /// A model picked from the listed ones.
    PickModel(Which, String),
    /// "Use a different model" for recommendations: off empties it.
    Separate(bool),
    FocusHours(String),
    Close,
    Save,
    Test(Which),
    WeekStart(Weekday),
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
    /// Light, dark or match system: applied on Save.
    Appearance(Appearance),
    /// Open or fold the Prompts section.
    TogglePrompts,
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

impl Which {
    pub const BOTH: [Which; 2] = [Which::Main, Which::Decision];
}

/// One value for each block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PerBlock<T> {
    pub main: T,
    pub decision: T,
}

impl<T> PerBlock<T> {
    pub fn get(&self, which: Which) -> &T {
        match which {
            Which::Main => &self.main,
            Which::Decision => &self.decision,
        }
    }

    pub fn get_mut(&mut self, which: Which) -> &mut T {
        match which {
            Which::Main => &mut self.main,
            Which::Decision => &mut self.decision,
        }
    }
}

/// One block's model list: the last fetch's result and whether a fetch is
/// in flight.
#[derive(Debug, Clone, Default)]
pub struct ModelList {
    /// `Ok(models)` (may be empty when the endpoint can't say) or
    /// `Err(message)`. Only Ollama says what kind each model is; others
    /// list `Chat`.
    pub result: Option<Result<Vec<OllamaModel>, String>>,
    pub loading: bool,
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
    /// Send this message back a moment from now (`Message::UrlSettled`).
    Later(Message),
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
    /// Each block's model list.
    pub models: PerBlock<ModelList>,
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

/// An appearance choice with a `Display` impl, for `pick_list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ThemeChoice(Appearance);

const THEME_CHOICES: [ThemeChoice; 3] = [
    ThemeChoice(Appearance::System),
    ThemeChoice(Appearance::Light),
    ThemeChoice(Appearance::Dark),
];

impl fmt::Display for ThemeChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            Appearance::System => "Match system",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
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
    models: PerBlock<ModelList>,
    /// The endpoint of each block's last model-list request, to hide a list
    /// that belongs to an endpoint the form no longer names.
    listed_for: PerBlock<Option<(Provider, String)>>,
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
    /// The Prompts section is unfolded. Shut by default: they are long.
    prompts_open: bool,
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
            models: PerBlock::default(),
            listed_for: PerBlock::default(),
            decision_on: false,
            ollama: None,
            ollama_prefilled: false,
            tested: Which::Main,
            tested_llm: None,
            password_prompt: false,
            prompts_open: false,
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
        self.ollama = data.ollama;
        self.prefill_ollama();
    }

    pub fn update(&mut self, message: Message) -> Action {
        let form = &mut self.form;
        match message {
            Message::Provider(w, p) => {
                form.endpoint_mut(w).set_provider(p);
                return self.update(Message::LoadModels(w));
            }
            Message::Separate(on) => {
                self.decision_on = on;
                if !on {
                    form.decision = Endpoint::default();
                } else if form.decision == Endpoint::default() {
                    return self.update(Message::Provider(Which::Decision, Provider::Ollama));
                }
            }
            Message::BaseUrl(w, s) => {
                form.endpoint_mut(w).base_url = s.clone();
                return Action::Later(Message::UrlSettled(w, s));
            }
            Message::UrlSettled(w, s) => {
                if self.form.endpoint(w).base_url == s {
                    return self.update(Message::LoadModels(w));
                }
            }
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
                    *self.listed_for.get_mut(w) = Some((provider, llm.base_url.clone()));
                    return Action::ListModels(llm, w);
                }
            }
            Message::Close => {
                self.on_hidden();
                return Action::Close;
            }
            Message::WeekStart(d) => form.week_start = d,
            Message::Appearance(choice) => form.appearance = choice,
            Message::TogglePrompts => self.prompts_open = !self.prompts_open,
            Message::AppIcon(choice) => form.app_icon = choice,
            Message::Accent(accent) => form.accent = accent,
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
            self.appearance_section(),
            self.model_section(),
            self.recommendations_section(),
            self.week_section(),
            self.data_section(),
            self.prompts_section(),
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

    fn week_section(&self) -> Element<'_, Message> {
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
                    .on_toggle_maybe((!last).then_some(move |on| Message::WorkingDay(i, on)))
                    // The locked box keeps the look of a ticked one.
                    .style(|theme, status| {
                        checkbox::primary(
                            theme,
                            match status {
                                checkbox::Status::Disabled { is_checked } => {
                                    checkbox::Status::Active { is_checked }
                                }
                                status => status,
                            },
                        )
                    }),
            );
        }
        // Older settings may hold no working day: mark the row, as other
        // fields that stop a save are marked.
        let days_label = text("Working days").size(theme::TEXT_SMALL);
        let days_label = if ticked == 0 {
            days_label.style(theme::danger)
        } else {
            days_label
        };
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
            "Week",
            column![
                labelled(
                    "Week starts on",
                    day_picker(f.week_start, Message::WeekStart)
                ),
                column![days_label, days].spacing(theme::SPACING / 2.0),
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
        // Folds like Today's No date section.
        let arrow = if self.prompts_open { "▾" } else { "▸" };
        let heading = button(text(format!("{arrow}  Prompts")).size(theme::TEXT_SMALL))
            .padding([4, 0])
            .style(theme::text_button)
            .on_press(Message::TogglePrompts);
        let mut body = column![heading].spacing(theme::SPACING);
        if self.prompts_open {
            body = body.push(
                column![
                    editor("Work summary", PromptKind::Summary),
                    editor("Recommended tasks", PromptKind::Plan),
                ]
                .spacing(theme::SPACING * 1.25),
            );
        }
        body.into()
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
        let mode = pick_list(
            THEME_CHOICES,
            Some(ThemeChoice(self.form.appearance)),
            |c: ThemeChoice| Message::Appearance(c.0),
        )
        .padding([8, 12])
        .text_size(theme::TEXT_BODY)
        .width(180)
        .style(picker);
        let theme_row = row![
            column![text("Theme").size(theme::TEXT_SMALL), mode].spacing(theme::SPACING / 3.0),
            labelled("Color", swatches),
        ]
        .spacing(theme::SPACING * 2.0);
        section(
            "Appearance",
            column![theme_row, labelled("App icon", icon)].spacing(theme::SPACING),
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

fn input<'a>(placeholder: &str, value: &str) -> text_input::TextInput<'a, Message> {
    text_input(placeholder, value)
        .padding([8, 12])
        .size(theme::TEXT_BODY)
        .style(theme::input)
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

#[cfg(test)]
mod test_util {
    use super::*;

    pub(super) fn data(settings: Settings, key_state: KeyState) -> SettingsData {
        SettingsData {
            settings,
            data_dir: PathBuf::from("/tmp/fortnight-data"),
            key_state,
            decision_key_state: KeyState::NoKey,
            test_result: None,
            testing: false,
            key_message: None,
            models: PerBlock::default(),
            ollama: None,
        }
    }

    pub(super) fn loaded(key_state: KeyState) -> SettingsView {
        let mut v = SettingsView::new();
        v.load(data(Settings::default(), key_state));
        v
    }

    pub(super) fn typed(v: &mut SettingsView, field: SecretField, s: &str) {
        v.update(Message::Secret(field, Typed(s.into())));
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;

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
    fn debug_never_prints_secrets() {
        let mut v = loaded(KeyState::NoKey);
        let m = Message::Secret(SecretField::ApiKey, Typed("sk-secret".into()));
        assert!(!format!("{m:?}").contains("sk-secret"));
        v.update(m);
        assert!(!format!("{v:?}").contains("sk-secret"));
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
    fn prompts_start_folded_and_toggle_without_dirtying() {
        let mut v = loaded(KeyState::NoKey);
        assert!(!v.prompts_open);
        v.update(Message::TogglePrompts);
        assert!(v.prompts_open);
        assert!(!v.is_dirty());
        v.update(Message::TogglePrompts);
        assert!(!v.prompts_open);
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
}
