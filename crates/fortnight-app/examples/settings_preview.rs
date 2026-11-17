//! Visual check for the Settings view and the unlock prompt, with fixture
//! data and no store, vault or network.
//!
//! `cargo run -p fortnight-app --example settings_preview`
//!
//! Env:
//! - `PREVIEW_DARK=1` forces the dark theme.
//! - `PREVIEW_SCREEN=unlock` shows the unlock prompt (default: settings).
//! - `PREVIEW_KEY=none|locked|unlocked` sets the key state (default: none).
//! - `FORTNIGHT_SCREENSHOT=<path>` screenshots after two seconds and exits.
//!
//! The fake handlers: Unlock accepts the password `password`; SetKey,
//! ChangePassword, RemoveKey and Lock change the state at once; Test replies
//! after a short delay (an error if the URL contains "fail").

use fortnight_app::theme::theme_for;
use fortnight_app::views::settings::{
    Action, KeyState, Message as SettingsMessage, SettingsData, SettingsView, UnlockAction,
    UnlockMessage, UnlockView,
};
use fortnight_core::domain::{LlmSettings, Settings};
use iced::{Element, Task as Command, Theme};
use secrecy::ExposeSecret;
use std::path::PathBuf;

pub fn main() -> iced::Result {
    iced::application(Preview::new, Preview::update, Preview::view)
        .title("Settings preview")
        .theme(Preview::theme)
        .window_size((900.0, 900.0))
        .run()
}

struct Preview {
    settings_view: SettingsView,
    unlock: Option<UnlockView>,
    data: SettingsData,
    dark: bool,
}

#[derive(Debug, Clone)]
enum Message {
    Settings(SettingsMessage),
    Unlock(UnlockMessage),
    Tested(Result<String, String>),
    Shoot,
    Shot(iced::window::Screenshot),
}

impl Preview {
    fn new() -> (Self, Command<Message>) {
        let key_state = match std::env::var("PREVIEW_KEY").as_deref() {
            Ok("locked") => KeyState::Locked,
            Ok("unlocked") => KeyState::Unlocked,
            _ => KeyState::NoKey,
        };
        let settings = Settings {
            llm: LlmSettings {
                base_url: "https://api.openai.com/v1".into(),
                model: "gpt-4o-mini".into(),
            },
            summary_instructions: "Under 150 words, bullet points.".into(),
            ..Settings::default()
        };
        let data = SettingsData {
            settings,
            data_dir: PathBuf::from("/Users/me/Apps/fortnight-data"),
            key_state,
            test_result: (key_state == KeyState::Unlocked)
                .then(|| Ok("OK — ready when you are.".to_string())),
            testing: false,
            key_message: None,
        };
        let unlock =
            (std::env::var("PREVIEW_SCREEN").as_deref() == Ok("unlock")).then(UnlockView::new);
        let mut settings_view = SettingsView::new();
        settings_view.load(data.clone());
        let preview = Preview {
            settings_view,
            unlock,
            data,
            dark: std::env::var_os("PREVIEW_DARK").is_some(),
        };
        let focus = UnlockView::focus().map(Message::Unlock);
        (preview, Command::batch([focus, shoot_after_delay()]))
    }

    fn theme(&self) -> Option<Theme> {
        Some(theme_for(self.dark))
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        let mut task = Command::none();
        match message {
            Message::Unlock(m) => {
                if let Some(unlock) = self.unlock.as_mut() {
                    match unlock.update(m) {
                        UnlockAction::None => {}
                        UnlockAction::Unlock(pw) if pw.expose_secret() == "password" => {
                            self.data.key_state = KeyState::Unlocked;
                            self.unlock = None;
                        }
                        UnlockAction::Unlock(_) => unlock.set_error("wrong password".into()),
                        UnlockAction::Skip => {
                            self.data.key_state = KeyState::Locked;
                            self.unlock = None;
                        }
                    }
                }
            }
            Message::Settings(m) => task = self.handle(m),
            Message::Tested(result) => {
                self.data.testing = false;
                self.data.test_result = Some(result);
            }
            Message::Shoot => {
                return iced::window::latest()
                    .and_then(iced::window::screenshot)
                    .map(Message::Shot);
            }
            Message::Shot(shot) => return save_screenshot(shot),
        }
        self.settings_view.load(self.data.clone());
        task
    }

    fn handle(&mut self, m: SettingsMessage) -> Command<Message> {
        let d = &mut self.data;
        match self.settings_view.update(m) {
            Action::None => {}
            Action::Save(s) => d.settings = s,
            Action::Test(llm) => {
                d.testing = true;
                d.test_result = None;
                return Command::perform(
                    tokio::time::sleep(std::time::Duration::from_millis(700)),
                    move |_| {
                        Message::Tested(if llm.base_url.contains("fail") {
                            Err("connection refused".into())
                        } else {
                            Ok(format!("Hello from {}", llm.model))
                        })
                    },
                );
            }
            Action::SetKey { .. } => {
                d.key_state = KeyState::Unlocked;
                d.key_message = Some(Ok("Key saved.".into()));
            }
            Action::ChangePassword { old, .. } => {
                d.key_message = Some(if old.expose_secret() == "password" {
                    Ok("Password changed.".into())
                } else {
                    Err("wrong password".into())
                });
            }
            Action::RemoveKey => {
                d.key_state = KeyState::NoKey;
                d.key_message = Some(Ok("Key removed.".into()));
            }
            Action::Lock => {
                d.key_state = KeyState::Locked;
                d.key_message = None;
            }
            Action::Unlock(pw) => {
                if pw.expose_secret() == "password" {
                    d.key_state = KeyState::Unlocked;
                    d.key_message = None;
                } else {
                    d.key_message = Some(Err("wrong password".into()));
                }
            }
        }
        Command::none()
    }

    fn view(&self) -> Element<'_, Message> {
        match &self.unlock {
            Some(unlock) => unlock.view().map(Message::Unlock),
            None => self.settings_view.view().map(Message::Settings),
        }
    }
}

fn shoot_after_delay() -> Command<Message> {
    if std::env::var_os("FORTNIGHT_SCREENSHOT").is_none() {
        return Command::none();
    }
    Command::perform(
        tokio::time::sleep(std::time::Duration::from_secs(2)),
        |_| Message::Shoot,
    )
}

fn save_screenshot(shot: iced::window::Screenshot) -> Command<Message> {
    if let Some(path) = std::env::var_os("FORTNIGHT_SCREENSHOT") {
        let mut bytes = format!("{} {}\n", shot.size.width, shot.size.height).into_bytes();
        bytes.extend_from_slice(&shot.rgba);
        if let Err(e) = std::fs::write(&path, bytes) {
            eprintln!("screenshot: {e}");
        }
    }
    iced::exit()
}
