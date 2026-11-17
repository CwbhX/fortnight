//! The API keys: the main and the decision model's key in memory, whether
//! each key file exists, the "key is locked for this endpoint" rule, the LLM
//! clients built from them, and Lock and unlock (including saving and
//! removing a key from Settings).

use crate::{Message, Ready};
use fortnight_app::views::settings::{self, KeyState};
use fortnight_core::decision::DecisionClient;
use fortnight_core::domain::{LlmSettings, Protocol};
use fortnight_core::fallback;
use fortnight_core::llm::{self, LlmClient, LlmConfig};
use fortnight_core::secure;
use fortnight_core::vault::{self, VaultError};
use iced::Task;
use secrecy::SecretString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The saved keys: whether each key file exists, and each key while it is
/// unlocked.
pub(crate) struct Keys {
    key_exists: bool,
    api_key: Option<SecretString>,
    /// The decision model's own key (`vault::decision_key_path`), under the
    /// same password: unlocked and locked together with `api_key`.
    decision_key_exists: bool,
    decision_key: Option<SecretString>,
}

impl Keys {
    /// Both keys locked, noting which key files exist in `dir`.
    pub(crate) fn open(dir: &Path) -> Keys {
        let key_exists = vault::key_path(dir).exists();
        let decision_key_exists = vault::decision_key_path(dir).exists();
        Keys {
            key_exists,
            api_key: None,
            decision_key_exists,
            decision_key: None,
        }
    }

    /// Either key is unlocked.
    pub(crate) fn any_open(&self) -> bool {
        self.api_key.is_some() || self.decision_key.is_some()
    }

    /// The saved key is locked and `llm` needs it. A keyless endpoint never
    /// needs it; a local one (Ollama, LM Studio) works without it and gets
    /// it only if it is unlocked.
    fn locked_for(&self, llm: &LlmSettings) -> bool {
        key_locked(self.key_exists, &self.api_key, llm)
    }

    pub(super) fn client_for(
        &self,
        llm: &LlmSettings,
    ) -> Result<(Arc<dyn LlmClient>, Option<String>), String> {
        if self.locked_for(llm) {
            return Err("The API key is locked. Unlock it first.".into());
        }
        let config = LlmConfig::from_settings(llm, self.api_key.clone());
        let host = config.endpoint_host();
        let client = llm::connect(llm.protocol, config).map_err(|e| e.to_string())?;
        Ok((client, host))
    }

    /// The client for the decision model (`Settings::decision_llm`) with
    /// these fields, and its host. It uses its own key if one is saved, and
    /// otherwise the main key only on the main endpoint's host
    /// (`fallback::decision_config`); `main` is the main endpoint's fields.
    pub(super) fn decision_client_for(
        &self,
        llm: &LlmSettings,
        main: &LlmSettings,
    ) -> Result<(Arc<dyn LlmClient>, Option<String>), String> {
        if key_locked(self.decision_key_exists, &self.decision_key, llm) {
            return Err("The decision model's API key is locked. Unlock it first.".into());
        }
        let config =
            fallback::decision_config(llm, main, self.api_key.clone(), self.decision_key.clone());
        let host = config.endpoint_host();
        let client = llm::connect(llm.protocol, config).map_err(|e| e.to_string())?;
        Ok((client, host))
    }

    pub(super) fn key_state(&self) -> KeyState {
        key_state(self.key_exists, &self.api_key)
    }

    pub(super) fn decision_key_state(&self) -> KeyState {
        key_state(self.decision_key_exists, &self.decision_key)
    }

    /// Both keys together, for the top bar's lock: locked if either saved
    /// key is.
    pub(crate) fn any_key_state(&self) -> KeyState {
        match (self.key_state(), self.decision_key_state()) {
            (KeyState::NoKey, KeyState::NoKey) => KeyState::NoKey,
            (KeyState::Locked, _) | (_, KeyState::Locked) => KeyState::Locked,
            _ => KeyState::Unlocked,
        }
    }
}

impl Ready {
    /// Why the LLM can't be used right now, if it can't.
    pub(super) fn blocked(&self) -> Option<String> {
        if !self.settings.llm.is_configured() {
            Some("no endpoint is set up in Settings.".into())
        } else if self.keys.locked_for(&self.settings.llm) {
            Some("the API key is locked.".into())
        } else {
            None
        }
    }

    /// The saved recommendations model is an Ollama decision model.
    pub(crate) fn decision_active(&self) -> bool {
        let d = &self.settings.decision_llm;
        d.protocol == Protocol::OllamaDecision && d.is_configured()
    }

    /// The saved decision model's client, for priority suggestions.
    pub(super) fn decision_model(&self) -> Option<DecisionClient> {
        if !self.decision_active() {
            return None;
        }
        let config = fallback::decision_config(
            &self.settings.decision_llm,
            &self.settings.llm,
            self.keys.api_key.clone(),
            self.keys.decision_key.clone(),
        );
        // Suggestions are only worth having quickly.
        DecisionClient::new(config)
            .ok()
            .map(|c| c.with_timeout(Duration::from_secs(15)))
    }

    /// The client a Settings Test or Load models button asks for.
    pub(super) fn settings_client(
        &self,
        llm: &LlmSettings,
        which: settings::Which,
    ) -> Result<(Arc<dyn LlmClient>, Option<String>), String> {
        match which {
            settings::Which::Main => self.keys.client_for(llm),
            settings::Which::Decision => self.keys.decision_client_for(llm, &self.settings.llm),
        }
    }

    /// Forgets the key and stops every request that holds a copy of it.
    /// Reports being generated are dropped (not stored); the schedule
    /// generates them again after the next unlock.
    pub(crate) fn lock(&mut self) {
        self.keys.api_key = None; // dropping the SecretString zeroizes it
        self.keys.decision_key = None;
        self.reports.abort_all();
        for handle in self.key_requests.drain(..) {
            handle.abort();
        }
        self.later_days.abort_all();
        self.suggest.cancel();
        self.testing = false;
        self.listing_models = false;
        self.key_message = None;
    }

    /// Opens both saved keys with `password`. Fails only if it opens
    /// neither.
    pub(crate) fn unlock(&mut self, password: SecretString) -> Task<Message> {
        let main = self
            .keys
            .key_exists
            .then(|| vault::key_path(&self.data_dir));
        let decision = self
            .keys
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

    /// The unlock prompt's answer: the keys it opened, or why not.
    pub(crate) fn unlocked(
        &mut self,
        result: Result<(Option<SecretString>, Option<SecretString>), VaultError>,
    ) -> Task<Message> {
        match result {
            Ok((main, decision)) => {
                if main.is_some() {
                    self.keys.api_key = main;
                }
                if decision.is_some() {
                    self.keys.decision_key = decision;
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

    /// A key `set_key` saved, or why it couldn't.
    pub(crate) fn key_saved(&mut self, result: Result<SavedKey, VaultError>) -> Task<Message> {
        match result {
            Ok(saved) => {
                let (mine, other) = match saved.which {
                    settings::Which::Main => {
                        self.keys.key_exists = true;
                        (&mut self.keys.api_key, &mut self.keys.decision_key)
                    }
                    settings::Which::Decision => {
                        self.keys.decision_key_exists = true;
                        (&mut self.keys.decision_key, &mut self.keys.api_key)
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

    /// Saves `api_key` as the `which` key under `password` (Settings).
    pub(super) fn set_key(
        &mut self,
        api_key: SecretString,
        password: SecretString,
        which: settings::Which,
    ) -> Task<Message> {
        let main_path = vault::key_path(&self.data_dir);
        let decision_path = vault::decision_key_path(&self.data_dir);
        self.key_message = None;
        let (path, other) = match which {
            settings::Which::Main => (
                main_path,
                self.keys.decision_key_exists.then_some(decision_path),
            ),
            settings::Which::Decision => (decision_path, self.keys.key_exists.then_some(main_path)),
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

    /// Removes the `which` key file and forgets the key (Settings).
    pub(super) fn remove_key(&mut self, which: settings::Which) -> Task<Message> {
        let main_path = vault::key_path(&self.data_dir);
        let decision_path = vault::decision_key_path(&self.data_dir);
        let (path, exists, key) = match which {
            settings::Which::Main => (main_path, &mut self.keys.key_exists, &mut self.keys.api_key),
            settings::Which::Decision => (
                decision_path,
                &mut self.keys.decision_key_exists,
                &mut self.keys.decision_key,
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
}

/// A key `SetKey` saved, and the other key if the password opened it.
#[derive(Debug, Clone)]
pub(crate) struct SavedKey {
    which: settings::Which,
    key: SecretString,
    other: Option<SecretString>,
}

/// The saved key is locked (`exists` but not open) and `llm` needs it
/// (`secure::needs_saved_key`).
fn key_locked(exists: bool, key: &Option<SecretString>, llm: &LlmSettings) -> bool {
    exists && key.is_none() && secure::needs_saved_key(llm)
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
