//! The Model and Recommendations blocks: provider, endpoint, the model
//! list and its choices, the first-run Ollama prefill and the endpoint grid.

use fortnight_core::domain::{LlmSettings, Protocol};
use fortnight_core::ollama::{self, DEFAULT_BASE_URL as OLLAMA_BASE_URL, ModelKind, OllamaModel};
use fortnight_core::secure;
use iced::widget::{button, column, pick_list, row, text};
use iced::{Center, Element, Fill, Theme};
use std::fmt;

use super::{KeyState, Message, SettingsView, Which, error_text, field_row, input, picker};
use crate::theme;

/// What the model-list area shows.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ModelsState<'a> {
    Hidden,
    /// The provider needs an API key and none is saved: nothing to ask.
    NeedsKey,
    Busy,
    /// The endpoint answered but listed no models.
    Empty,
    Error(&'a str),
    /// The models this block can use (no decision models for Model).
    List(Vec<OllamaModel>),
}

/// A listed model in the Choose list: its name, tagged with what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModelChoice(OllamaModel);

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
pub(super) fn is_local_ollama(url: &str) -> bool {
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
    pub(super) const ALL: [Provider; 3] = [Provider::Ollama, Provider::OpenAi, Provider::Anthropic];

    /// Ollama's is the one on this machine (`OLLAMA_HOST` if set).
    pub(super) fn default_url(self) -> String {
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
pub(super) struct Endpoint {
    pub(super) provider: Provider,
    pub(super) base_url: String,
    pub(super) model: String,
    /// An Ollama decision model (`Protocol::OllamaDecision`), only ever in
    /// the Recommendations block.
    pub(super) decision: bool,
}

impl Endpoint {
    /// Keyless endpoints, and older settings on Ollama's port, show as
    /// Ollama.
    pub(super) fn from_llm(llm: &LlmSettings) -> Self {
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

    pub(super) fn llm(&self) -> LlmSettings {
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
    pub(super) fn set_provider(&mut self, provider: Provider) {
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

/// First `max` characters of the first line, with an ellipsis if cut.
pub(super) fn shorten(s: &str, max: usize) -> String {
    let line = s.trim().lines().next().unwrap_or("");
    if line.chars().count() > max || s.trim().lines().count() > 1 {
        let cut: String = line.chars().take(max).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

/// Success text: the theme's success colour.
pub(super) fn success(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(theme.palette().success),
    }
}

impl SettingsView {
    /// First run: no model set up and Ollama is running, so the Model block
    /// starts on Ollama with its first chat model (saved only on Save).
    pub(super) fn prefill_ollama(&mut self) {
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
    pub(super) fn kind_of_model(&self, which: Which, name: &str) -> Option<ModelKind> {
        match self.models_state(which) {
            ModelsState::List(models) => models
                .iter()
                .find(|m| m.name == name.trim())
                .map(|m| m.kind),
            _ => None,
        }
    }

    pub(super) fn can_test(&self, which: Which) -> bool {
        self.form.endpoint(which).llm().is_configured() && !self.testing
    }

    /// Whether the last Test result is shown under `which`: it was for this
    /// block, it is in, and the form still names the endpoint it tested.
    fn shows_test_result(&self, which: Which) -> bool {
        self.tested == which
            && !self.testing
            && self.tested_llm.as_ref() == Some(&self.form.endpoint(which).llm())
    }

    /// A request in flight doesn't stop another: `main.rs` drops the older
    /// one.
    pub(super) fn can_list_models(&self, which: Which) -> bool {
        !self.form.endpoint(which).base_url.trim().is_empty() && !self.needs_key(which)
    }

    /// The block's endpoint takes a saved key and there is none it could
    /// use. The recommendations block may borrow the main key
    /// (`fallback::decision_config`).
    fn needs_key(&self, which: Which) -> bool {
        let none = match which {
            Which::Main => self.key_state == KeyState::NoKey,
            Which::Decision => {
                self.decision_key_state == KeyState::NoKey && self.key_state == KeyState::NoKey
            }
        };
        none && secure::needs_saved_key(&self.form.endpoint(which).llm())
    }

    /// The models this block can pick from: its last fetch, or the local
    /// Ollama check when the block is on Ollama's default address. A list
    /// for another endpoint isn't shown.
    pub(super) fn models_state(&self, which: Which) -> ModelsState<'_> {
        let e = self.form.endpoint(which);
        let usable = |models: &[OllamaModel]| -> Vec<OllamaModel> {
            models
                .iter()
                .filter(|m| which == Which::Decision || m.kind != ModelKind::Decision)
                .cloned()
                .collect()
        };
        let listed_here = self
            .listed_for
            .get(which)
            .as_ref()
            .is_some_and(|(provider, url)| *provider == e.provider && *url == e.base_url.trim());
        let list = self.models.get(which);
        if listed_here {
            if list.loading {
                return ModelsState::Busy;
            }
            match &list.result {
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
        if self.needs_key(which) {
            return ModelsState::NeedsKey;
        }
        ModelsState::Hidden
    }

    /// Provider, URL, model, API key (when the provider takes one) and Test
    /// for one endpoint, as label/field rows.
    pub(super) fn endpoint_grid(&self, which: Which) -> Element<'_, Message> {
        let f = self.form.endpoint(which);
        let picker_list = pick_list(Provider::ALL, Some(f.provider), move |p| {
            Message::Provider(which, p)
        })
        .padding(theme::DROPDOWN_PADDING)
        .handle(pick_list::Handle::None)
        .text_size(theme::TEXT_BODY)
        .width(240)
        .style(picker);
        let mut provider = row![theme::dropdown(picker_list)]
            .spacing(theme::SPACING)
            .align_y(Center);
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

        let model = self.model_field(which);

        let testing_here = self.testing && self.tested == which;
        let test =
            button(text(if testing_here { "Testing…" } else { "Test" }).size(theme::TEXT_BODY))
                .padding([8, 14])
                .style(theme::secondary_button)
                .on_press_maybe(self.can_test(which).then_some(Message::Test(which)));
        let mut test_row = row![test].spacing(theme::SPACING).align_y(Center);
        if self.shows_test_result(which) {
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

    /// The Model field: a list the endpoint fills, with the chosen model in
    /// it even when the endpoint doesn't list it. Only an endpoint that
    /// lists no models gets a text field.
    fn model_field(&self, which: Which) -> Element<'_, Message> {
        let f = self.form.endpoint(which);
        let state = self.models_state(which);
        if state == ModelsState::Empty {
            return input("Model name", &f.model)
                .on_input(move |s| Message::Model(which, s))
                .into();
        }
        let placeholder = match &state {
            ModelsState::Busy => "Loading…",
            ModelsState::NeedsKey => "Needs an API key",
            _ => "Choose",
        };
        let error = match &state {
            ModelsState::Error(e) => Some(shorten(e, 120)),
            _ => None,
        };
        let mut choices: Vec<ModelChoice> = match state {
            ModelsState::List(models) => models.into_iter().map(ModelChoice).collect(),
            _ => Vec::new(),
        };
        let current = f.model.trim();
        if !current.is_empty() && !choices.iter().any(|c| c.0.name == current) {
            choices.insert(
                0,
                ModelChoice(OllamaModel {
                    name: current.to_string(),
                    kind: if f.decision {
                        ModelKind::Decision
                    } else {
                        ModelKind::Chat
                    },
                }),
            );
        }
        let selected = choices.iter().find(|c| c.0.name == current).cloned();
        let list = pick_list(choices, selected, move |c: ModelChoice| {
            Message::PickModel(which, c.0.name)
        })
        .placeholder(placeholder)
        .padding(theme::DROPDOWN_PADDING)
        .handle(pick_list::Handle::None)
        .text_size(theme::TEXT_BODY)
        .width(Fill)
        .style(picker);
        let list = theme::dropdown(list);
        let Some(e) = error else {
            return list;
        };
        let retry = button(text("Retry").size(theme::TEXT_BODY))
            .padding([8, 14])
            .style(theme::secondary_button)
            .on_press_maybe(
                self.can_list_models(which)
                    .then_some(Message::LoadModels(which)),
            );
        column![
            row![list, retry]
                .spacing(theme::SPACING / 2.0)
                .align_y(Center),
            error_text(e),
        ]
        .spacing(theme::SPACING / 3.0)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::super::*;
    use super::*;

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
        // The list belongs to the recommendations block only (Model, on
        // OpenAI with no key, has nothing to list).
        assert_eq!(v.models_state(Which::Main), ModelsState::NeedsKey);
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

    /// Settings data whose main block's list is `models`.
    fn listed(models: Result<Vec<OllamaModel>, String>) -> SettingsData {
        let mut d = data(Settings::default(), KeyState::NoKey);
        d.models.main.result = Some(models);
        d
    }

    #[test]
    fn list_models_carries_current_fields() {
        let mut v = loaded(KeyState::NoKey);
        // Needs a base URL.
        assert!(matches!(
            v.update(Message::LoadModels(Which::Main)),
            Action::None
        ));
        // Picking a provider loads its list, once there is a key for it.
        assert!(matches!(
            v.update(Message::Provider(Which::Main, Provider::Anthropic)),
            Action::None
        ));
        assert_eq!(v.models_state(Which::Main), ModelsState::NeedsKey);
        v.load(data(Settings::default(), KeyState::Unlocked));
        assert_eq!(v.models_state(Which::Main), ModelsState::Hidden);
        let Action::ListModels(llm, Which::Main) =
            v.update(Message::Provider(Which::Main, Provider::Anthropic))
        else {
            panic!("expected ListModels");
        };
        assert_eq!(llm.protocol, Protocol::Anthropic);
        assert_eq!(llm.base_url, "https://api.anthropic.com");
        // A fetch in flight shows as busy and doesn't stop another.
        let mut d = data(Settings::default(), KeyState::Unlocked);
        d.models.main.loading = true;
        v.load(d);
        assert_eq!(v.models_state(Which::Main), ModelsState::Busy);
        assert!(matches!(
            v.update(Message::LoadModels(Which::Main)),
            Action::ListModels(_, Which::Main)
        ));
    }

    #[test]
    fn a_url_edit_loads_models_once_it_settles() {
        let mut v = loaded(KeyState::NoKey);
        let Action::Later(first) = v.update(Message::BaseUrl(
            Which::Main,
            "http://localhost:8080".into(),
        )) else {
            panic!("expected Later");
        };
        let Action::Later(second) = v.update(Message::BaseUrl(
            Which::Main,
            "http://localhost:8080/v1".into(),
        )) else {
            panic!("expected Later");
        };
        // The earlier edit was overtaken.
        assert!(matches!(v.update(first), Action::None));
        let Action::ListModels(llm, Which::Main) = v.update(second) else {
            panic!("expected ListModels");
        };
        assert_eq!(llm.base_url, "http://localhost:8080/v1");
    }

    #[test]
    fn recommendations_can_list_with_the_main_key() {
        let mut v = loaded(KeyState::Locked);
        v.update(Message::Separate(true));
        v.update(Message::Provider(Which::Decision, Provider::OpenAi));
        assert!(v.can_list_models(Which::Decision));
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::Separate(true));
        v.update(Message::Provider(Which::Decision, Provider::OpenAi));
        assert_eq!(v.models_state(Which::Decision), ModelsState::NeedsKey);
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

        v.load(listed(Ok(vec![])));
        assert_eq!(v.models_state(Which::Main), ModelsState::Empty);

        v.load(listed(Err("404 Not Found".into())));
        assert_eq!(
            v.models_state(Which::Main),
            ModelsState::Error("404 Not Found")
        );

        let ids = vec![chat("llama3.2"), chat("qwen3")];
        v.load(listed(Ok(ids.clone())));
        assert_eq!(v.models_state(Which::Main), ModelsState::List(ids.clone()));
        v.update(Message::PickModel(Which::Main, "qwen3".into()));
        assert_eq!(v.form.main.model, "qwen3");
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.llm.model, "qwen3");

        // The other block's list isn't this one's.
        v.update(Message::Separate(true));
        v.update(Message::BaseUrl(
            Which::Decision,
            "http://localhost:8080/v1".into(),
        ));
        assert_eq!(v.models_state(Which::Decision), ModelsState::Hidden);

        // A list for another endpoint is hidden.
        v.update(Message::BaseUrl(
            Which::Main,
            "http://localhost:9090/v1".into(),
        ));
        assert_eq!(v.models_state(Which::Main), ModelsState::Hidden);
        // Rendering works either way.
        let _ = v.view();
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
        let shown = |v: &SettingsView| v.shows_test_result(Which::Main);
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
}
