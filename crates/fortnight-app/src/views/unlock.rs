//! The unlock prompt, opened from the lock icon.

use iced::widget::{Space, button, column, container, operation, text, text_input};
use iced::{Element, Fill};
use secrecy::SecretString;
use std::fmt;

use super::settings::{Typed, take_secret, wipe};
use crate::{motion, theme};

pub(super) const UNLOCK_INPUT_ID: &str = "unlock-password";

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
    busy: bool,
}

impl fmt::Debug for UnlockView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnlockView")
            .field("password", &"***")
            .field("error", &self.error)
            .field("busy", &self.busy)
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

    /// Marks the prompt as busy while Argon2 derives the key, disabling the
    /// Unlock button and relabelling it "Unlocking…".
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    /// Shows an error such as "Wrong password" and clears the field.
    pub fn set_error(&mut self, message: String) {
        self.error = Some(message);
        self.busy = false;
        wipe(&mut self.password);
    }

    /// Wipes the typed password and clears the error, for the next time
    /// the prompt opens. `main.rs` calls it whenever the prompt closes.
    pub fn reset(&mut self) {
        wipe(&mut self.password);
        self.error = None;
        self.busy = false;
    }

    pub fn update(&mut self, message: UnlockMessage) -> UnlockAction {
        match message {
            UnlockMessage::Password(Typed(s)) => {
                if self.busy {
                    return UnlockAction::None;
                }
                wipe(&mut self.password);
                self.password = s;
                UnlockAction::None
            }
            UnlockMessage::Submit => {
                if self.busy || self.password.is_empty() {
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
            .on_submit_maybe((!self.busy).then_some(UnlockMessage::Submit))
            .padding([10, 14])
            .size(theme::TEXT_BODY + 1.0)
            .style(theme::input);
        let label = if self.busy { "Unlocking…" } else { "Unlock" };
        let unlock = button(container(text(label).size(theme::TEXT_BODY)).center_x(Fill))
            .width(Fill)
            .padding([10, 18])
            .style(theme::primary_button)
            .on_press_maybe(
                (!self.busy && !self.password.is_empty()).then_some(UnlockMessage::Submit),
            );
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

        let panel = container(card)
            .padding(theme::SPACING * 2.5)
            .max_width(380)
            .style(theme::overlay_panel);
        // The dark layer under it is the caller's, so it can fade out.
        container(motion::appear(panel).rise(motion::RISE))
            .center(Fill)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

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

    #[test]
    fn debug_never_prints_the_password() {
        let um = UnlockMessage::Password(Typed("pw-secret".into()));
        assert!(!format!("{um:?}").contains("pw-secret"));
        let mut u = UnlockView::new();
        u.update(um);
        assert!(!format!("{u:?}").contains("pw-secret"));
    }

    #[test]
    fn busy_state_blocks_submit_and_clears_on_error_or_reset() {
        let mut u = UnlockView::new();
        u.update(UnlockMessage::Password(Typed("hunter22".into())));
        u.set_busy(true);
        assert!(u.busy);
        // Submitting or editing while busy is ignored.
        assert!(matches!(
            u.update(UnlockMessage::Submit),
            UnlockAction::None
        ));
        assert!(matches!(
            u.update(UnlockMessage::Password(Typed("other".into()))),
            UnlockAction::None
        ));
        let _ = u.view();

        u.set_error("Wrong password".into());
        assert!(!u.busy);
        assert_eq!(u.error.as_deref(), Some("Wrong password"));

        u.set_busy(true);
        u.reset();
        assert!(!u.busy);
        assert!(u.error.is_none());
    }
}
