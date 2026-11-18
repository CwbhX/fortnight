//! The API key controls: typed secrets, the key form modes, the key
//! controls and the password pop-up, and saving a key.

use fortnight_core::vault::MIN_PASSWORD_LEN;
use iced::widget::{button, column, container, row, text, text_input};
use iced::{Center, Element, Fill};
use secrecy::SecretString;
use std::fmt;

use super::{Action, Message, SettingsView, Which, error_text, input, primary, secondary};
use crate::{motion, theme};

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

/// Which key form or confirmation is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum KeyMode {
    #[default]
    Idle,
    Replace,
    ConfirmRemove,
}

/// The secret fields. `Debug` never prints them.
#[derive(Default)]
pub(super) struct Secrets {
    pub(super) api_key: String,
    pub(super) decision_key: String,
    pub(super) password: String,
}

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secrets(***)")
    }
}

impl Secrets {
    pub(super) fn field(&mut self, field: SecretField) -> &mut String {
        match field {
            SecretField::ApiKey => &mut self.api_key,
            SecretField::DecisionKey => &mut self.decision_key,
            SecretField::Password => &mut self.password,
        }
    }

    pub(super) fn wipe_all(&mut self) {
        wipe(&mut self.api_key);
        wipe(&mut self.decision_key);
        wipe(&mut self.password);
    }
}

/// Clears a string, zeroising its buffer (via `SecretString`'s drop).
pub(crate) fn wipe(s: &mut String) {
    drop(SecretString::from(std::mem::take(s)));
}

/// Moves a string into a `SecretString`, leaving it empty.
pub(crate) fn take_secret(s: &mut String) -> SecretString {
    SecretString::from(std::mem::take(s))
}

/// Checks a new password.
pub(super) fn check_password(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(format!(
            "The password needs at least {MIN_PASSWORD_LEN} characters."
        ));
    }
    Ok(())
}

pub const PASSWORD_INPUT_ID: &str = "settings-key-password";

pub(super) fn secret_input<'a>(
    placeholder: &str,
    value: &str,
    field: SecretField,
) -> text_input::TextInput<'a, Message> {
    input(placeholder, value)
        .secure(true)
        .on_input(move |s| Message::Secret(field, Typed(s)))
}

pub(super) fn cancel_button<'a>() -> Element<'a, Message> {
    button(text("Cancel").size(theme::TEXT_BODY))
        .padding([8, 14])
        .style(theme::text_button)
        .on_press(Message::CancelKey)
        .into()
}

pub(super) fn remove_button<'a>(which: Which) -> Element<'a, Message> {
    button(text("Remove key").size(theme::TEXT_BODY))
        .padding([8, 14])
        .style(theme::danger_button)
        .on_press(Message::AskRemove(which))
        .into()
}

impl SettingsView {
    pub(super) fn open_key_form(&mut self, which: Which, mode: KeyMode) {
        self.close_key_form();
        self.key_target = which;
        self.key_mode = mode;
    }

    /// The key typed into `which`'s field.
    pub(super) fn typed_key(&self, which: Which) -> &str {
        match which {
            Which::Main => &self.secrets.api_key,
            Which::Decision => &self.secrets.decision_key,
        }
    }

    /// The state of `which`'s saved key.
    pub(super) fn key_state_of(&self, which: Which) -> KeyState {
        match which {
            Which::Main => self.key_state,
            Which::Decision => self.decision_key_state,
        }
    }

    pub(super) fn close_key_form(&mut self) {
        self.key_mode = KeyMode::Idle;
        self.key_error = None;
        self.password_prompt = false;
        self.secrets.wipe_all();
    }

    pub(super) fn save_key(&mut self) -> Action {
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

    /// The pop-up that asks for the password that encrypts the key.
    pub(super) fn password_view(&self) -> Element<'_, Message> {
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
        let panel = container(card)
            .padding(theme::SPACING * 2.0)
            .max_width(380)
            .style(theme::overlay_panel);
        // The dark layer under it is the caller's, so it can fade out.
        container(motion::appear(panel).rise(motion::RISE))
            .center(Fill)
            .into()
    }

    /// One key: its field and Save, or its state with Unlock, Replace and
    /// Remove. Errors about it show underneath.
    pub(super) fn key_controls(&self, which: Which) -> Element<'_, Message> {
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
    pub(super) fn key_input(&self, which: Which, cancellable: bool) -> Element<'_, Message> {
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
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::super::*;
    use super::*;
    use secrecy::ExposeSecret;

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
}
