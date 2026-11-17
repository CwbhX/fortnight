//! Settings view and the unlock prompt. Owner: agent I.

use fortnight_core::domain::{LlmSettings, Settings};
use iced::Element;
use iced::widget::text;
use secrecy::SecretString;
use std::path::PathBuf;

/// The state of the encrypted API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// No key file: the endpoint needs no key, or none is set yet.
    NoKey,
    Locked,
    Unlocked,
}

#[derive(Debug, Clone)]
pub enum Message {}

/// What `main.rs` must do. Contract: do not change without the orchestrator.
#[derive(Debug, Clone)]
pub enum Action {
    None,
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
}

#[derive(Debug, Default)]
pub struct SettingsView {}

impl SettingsView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces what the view shows. `main.rs` calls this after EVERY
    /// message, so it must be cheap. It must not discard unsaved form edits
    /// (typed URL, model, key, passwords) unless `settings` itself changed
    /// from what was last loaded.
    pub fn load(&mut self, _data: SettingsData) {}

    pub fn update(&mut self, message: Message) -> Action {
        match message {}
    }

    pub fn view(&self) -> Element<'_, Message> {
        text("Settings (agent I)").into()
    }
}

#[derive(Debug, Clone)]
pub enum UnlockMessage {}

/// What `main.rs` must do. Contract: do not change without the orchestrator.
#[derive(Debug, Clone)]
pub enum UnlockAction {
    None,
    Unlock(SecretString),
    /// Continue without unlocking; LLM features wait.
    Skip,
}

/// The password prompt shown at launch when a key file exists.
#[derive(Debug, Default)]
pub struct UnlockView {}

impl UnlockView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shows an error such as "Wrong password" and clears the field.
    pub fn set_error(&mut self, _message: String) {}

    pub fn update(&mut self, message: UnlockMessage) -> UnlockAction {
        match message {}
    }

    pub fn view(&self) -> Element<'_, UnlockMessage> {
        text("Unlock (agent I)").into()
    }
}
