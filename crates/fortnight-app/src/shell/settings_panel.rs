//! The Settings panel: opening and hiding it, and carrying out what it asks
//! for (save the settings, test an endpoint, list models, save, remove or
//! unlock a key), with the replies of its requests and the local Ollama
//! check.

use crate::{Message, Ready};
use fortnight_app::theme;
use fortnight_app::views::settings::{self, UnlockView};
use fortnight_core::domain::{Protocol, ReportKind};
use fortnight_core::llm::ChatRequest;
use fortnight_core::ollama::{self, ModelKind, OllamaModel};
use fortnight_core::report;
use fortnight_core::store::Store;
use iced::Task;
use iced::widget::operation;
use jiff::Timestamp;
use std::time::Duration;

impl Ready {
    /// Opens Settings and checks for a local Ollama.
    pub(crate) fn open_settings(&mut self) -> Task<Message> {
        self.settings_open = true;
        detect_ollama()
    }

    /// Hides Settings, by any route: typed secrets are wiped
    /// (`SettingsView::on_hidden`); form edits are kept.
    pub(crate) fn hide_settings(&mut self) {
        self.settings_open = false;
        self.settings_view.on_hidden();
    }

    pub(crate) fn settings_action(&mut self, action: settings::Action) -> Task<Message> {
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
                    self.reports.note_prompt_change(ReportKind::Summary, now);
                }
                if report::plan_prompt(&s) != report::plan_prompt(&self.settings) {
                    self.reports.note_prompt_change(ReportKind::Plan, now);
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
            } => self.set_key(api_key, password, which),
            settings::Action::RemoveKey(which) => self.remove_key(which),
            settings::Action::ShowUnlock => {
                self.unlocking = true;
                UnlockView::focus().map(Message::Unlock)
            }
            settings::Action::FocusPassword => operation::focus(settings::PASSWORD_INPUT_ID),
        }
    }

    /// A Test connection request answered (`Message::Tested`).
    pub(crate) fn tested(&mut self, result: Result<String, String>) -> Task<Message> {
        self.testing = false;
        if !self.listing_models {
            self.key_requests.clear();
        }
        self.test_result = Some(result);
        Task::none()
    }

    /// The local Ollama check answered (`Message::OllamaDetected`).
    pub(crate) fn ollama_detected(
        &mut self,
        result: Result<Vec<OllamaModel>, String>,
    ) -> Task<Message> {
        self.ollama = Some(result);
        Task::none()
    }

    /// A Load models request answered (`Message::Models`).
    pub(crate) fn models_listed(
        &mut self,
        result: Result<Vec<OllamaModel>, String>,
    ) -> Task<Message> {
        self.listing_models = false;
        if !self.testing {
            self.key_requests.clear();
        }
        self.models = Some(result);
        Task::none()
    }
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

/// Checks for Ollama on this machine (`Message::OllamaDetected`).
pub(crate) fn detect_ollama() -> Task<Message> {
    Task::perform(
        async {
            ollama::detect(&ollama::local_base_url(), Duration::from_secs(2))
                .await
                .map_err(|e| e.to_string())
        },
        Message::OllamaDetected,
    )
}
