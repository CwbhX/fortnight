//! The Settings panel: opening and hiding it, and carrying out what it asks
//! for (save the settings, test an endpoint, list models, save, remove or
//! unlock a key), with the replies of its requests and the local Ollama
//! check.

use crate::{Message, Ready};
use fortnight_app::theme;
use fortnight_app::views::settings::{self, UnlockView};
use fortnight_core::domain::{LlmSettings, Protocol, ReportKind};
use fortnight_core::llm::{ChatRequest, LlmClient};
use fortnight_core::ollama::{self, ModelKind, OllamaModel};
use fortnight_core::report;
use fortnight_core::store::Store;
use iced::Task;
use iced::widget::operation;
use jiff::Timestamp;
use std::sync::Arc;
use std::time::Duration;

impl Ready {
    /// Opens Settings, checks for a local Ollama and loads both blocks'
    /// model lists.
    pub(crate) fn open_settings(&mut self) -> Task<Message> {
        self.date_picker = None;
        self.settings_open = true;
        Task::batch([crate::unfocus(), detect_ollama(), load_models()])
    }

    /// Hides Settings, by any route: typed secrets are wiped
    /// (`SettingsView::on_hidden`); form edits are kept.
    pub(crate) fn hide_settings(&mut self) {
        self.settings_open = false;
        self.export_status = None;
        self.settings_view.on_hidden();
    }

    pub(crate) fn settings_action(&mut self, action: settings::Action) -> Task<Message> {
        match action {
            settings::Action::None => Task::none(),
            settings::Action::Close => {
                self.hide_settings();
                Task::none()
            }
            settings::Action::Save(mut s) => {
                s.window_size = self.window_size;
                let now = Timestamp::now();
                let summary_prompt_changed = report::summary_prompt(&s)
                    != report::summary_prompt(&self.settings)
                    || s.summary_instructions != self.settings.summary_instructions;
                let summary_changed = summary_prompt_changed || s.llm != self.settings.llm;
                // Recommend overdue tasks changes the prompt's rules too.
                let plan_prompt_changed = report::plan_prompt(&s)
                    != report::plan_prompt(&self.settings)
                    || s.recommend_overdue != self.settings.recommend_overdue;
                let plan_changed = plan_prompt_changed
                    || s.llm != self.settings.llm
                    || s.decision_llm != self.settings.decision_llm;
                match self.store.save_settings(&s) {
                    Ok(()) => {
                        if summary_prompt_changed {
                            self.reports.note_prompt_change(ReportKind::Summary, now);
                        }
                        if summary_changed {
                            self.reports.abort_kind(ReportKind::Summary);
                        }
                        if plan_prompt_changed {
                            self.reports.note_prompt_change(ReportKind::Plan, now);
                        }
                        if plan_changed {
                            self.reports.abort_kind(ReportKind::Plan);
                        }
                        theme::set_accent(s.accent);
                        theme::set_today_width(s.today_width);
                        // A fixed endpoint is tried again at once.
                        if s.llm != self.settings.llm
                            || s.decision_llm != self.settings.decision_llm
                        {
                            self.reset_backoff();
                        }
                        self.settings = s;
                        if plan_changed {
                            // Later days' lists are redone with the new
                            // prompt or model.
                            self.later_days.forget_all();
                        }
                        self.invalidate();
                        self.hide_settings();
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
                self.catch_up()
            }
            settings::Action::Test(llm, which) => match self.settings_client(&llm, which) {
                Ok((client, _)) => {
                    for handle in self.key_requests.drain(..) {
                        handle.abort();
                    }
                    self.test_seq += 1;
                    let seq = self.test_seq;
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
                        move |result| Message::Tested(seq, result),
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
            settings::Action::ListModels(llm, which) => self.list_models(llm, which),
            settings::Action::Later(m) => {
                Task::perform(tokio::time::sleep(Duration::from_millis(600)), move |_| {
                    Message::Settings(m)
                })
            }
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
            settings::Action::Export(format) => {
                self.export_data(format);
                Task::none()
            }
        }
    }

    /// Writes a full Markdown or JSON export into `<data_dir>/exports/`.
    fn export_data(&mut self, format: fortnight_core::export::ExportFormat) {
        let today = self.today();
        let reports = match self.store.all_reports() {
            Ok(r) => r,
            Err(e) => {
                self.export_status = Some(Err(format!("Could not read reports: {e}")));
                return;
            }
        };
        let content = fortnight_core::export::export_all(
            format,
            &self.tasks,
            &self.events,
            &reports,
            today,
            &self.tz,
        );
        let dir = self.data_dir.join("exports");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.export_status = Some(Err(format!("Could not create exports folder: {e}")));
            return;
        }
        let filename = format!("fortnight-export-{today}.{}", format.extension());
        let path = dir.join(&filename);
        match std::fs::write(&path, content) {
            Ok(()) => {
                self.export_status = Some(Ok(format!("Saved to {}", path.display())));
            }
            Err(e) => {
                self.export_status = Some(Err(format!("Could not write export: {e}")));
            }
        }
    }

    /// Fetches one block's model list (`Message::Models`), dropping the
    /// request it replaces.
    fn list_models(&mut self, llm: LlmSettings, which: settings::Which) -> Task<Message> {
        if let Some(old) = self.model_requests.get_mut(which).take() {
            old.abort();
        }
        *self.model_seq.get_mut(which) += 1;
        let seq = *self.model_seq.get(which);
        // Ollama (keyless, or a decision model): ask it directly, which
        // also says what each model can do. A keyless server that isn't
        // Ollama (LM Studio) answers the endpoint's own model list instead.
        let ollama = llm.keyless || llm.protocol == Protocol::OllamaDecision;
        let client = if llm.protocol == Protocol::OllamaDecision {
            None
        } else {
            match self.settings_client(&llm, which) {
                Ok((client, _)) => Some(client),
                Err(e) if !ollama => {
                    let list = self.models.get_mut(which);
                    list.result = Some(Err(e));
                    list.loading = false;
                    return Task::none();
                }
                Err(_) => None,
            }
        };
        let list = self.models.get_mut(which);
        list.result = None;
        list.loading = true;
        let from_endpoint = |client: Arc<dyn LlmClient>| async move {
            client
                .list_models()
                .await
                .map(chat_models)
                .map_err(|e| e.to_string())
        };
        let (task, handle) = Task::perform(
            async move {
                if !ollama {
                    let client = client.expect("a client unless Ollama");
                    return from_endpoint(client).await;
                }
                match ollama::detect(&llm.base_url, Duration::from_secs(10)).await {
                    Ok(models) => Ok(models),
                    Err(e) => match client {
                        Some(client) => from_endpoint(client).await,
                        None => Err(e.to_string()),
                    },
                }
            },
            move |result| Message::Models(which, seq, result),
        )
        .abortable();
        *self.model_requests.get_mut(which) = Some(handle);
        task
    }

    /// A Test connection request answered (`Message::Tested`).
    pub(crate) fn tested(&mut self, seq: u64, result: Result<String, String>) -> Task<Message> {
        if seq != self.test_seq {
            self.local = true;
            return Task::none();
        }
        self.testing = false;
        self.key_requests.clear();
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

    /// A model-list request answered (`Message::Models`).
    pub(crate) fn models_listed(
        &mut self,
        which: settings::Which,
        seq: u64,
        result: Result<Vec<OllamaModel>, String>,
    ) -> Task<Message> {
        if seq != *self.model_seq.get(which) {
            self.local = true;
            return Task::none();
        }
        *self.model_requests.get_mut(which) = None;
        let list = self.models.get_mut(which);
        list.loading = false;
        list.result = Some(result);
        Task::none()
    }

    /// Lock: stops the model-list requests (they hold the key).
    pub(crate) fn abort_model_requests(&mut self) {
        for which in settings::Which::BOTH {
            *self.model_seq.get_mut(which) += 1;
            if let Some(handle) = self.model_requests.get_mut(which).take() {
                handle.abort();
            }
            self.models.get_mut(which).loading = false;
        }
    }
}

/// Asks both Settings blocks to load their model lists (each does if it
/// can): when Settings opens and when a key is saved or unlocked.
pub(crate) fn load_models() -> Task<Message> {
    Task::batch(
        settings::Which::BOTH
            .map(|w| Task::done(Message::Settings(settings::Message::LoadModels(w)))),
    )
}

/// Model ids from an endpoint's model list, as chat models: sorted, without
/// the ones that can't chat (embeddings, speech, images, moderation, the
/// old completion models), which OpenAI's list is mostly made of.
fn chat_models(ids: Vec<String>) -> Vec<OllamaModel> {
    const NOT_CHAT: [&str; 12] = [
        "embed",
        "whisper",
        "tts",
        "dall-e",
        "moderation",
        "transcribe",
        "gpt-image",
        "davinci",
        "babbage",
        "sora",
        "realtime",
        "audio",
    ];
    let mut ids: Vec<String> = ids
        .into_iter()
        .filter(|id| {
            let id = id.to_ascii_lowercase();
            !NOT_CHAT.iter().any(|word| id.contains(word))
        })
        .collect();
    ids.sort();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_models_drops_what_cannot_chat_and_sorts() {
        let ids = [
            "gpt-4o",
            "text-embedding-3-small",
            "whisper-1",
            "dall-e-3",
            "gpt-4o-mini-tts",
            "omni-moderation-latest",
            "babbage-002",
            "gpt-4.1",
        ];
        let names: Vec<String> = chat_models(ids.map(String::from).to_vec())
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(names, ["gpt-4.1", "gpt-4o"]);
    }
}
