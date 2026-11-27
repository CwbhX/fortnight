//! Keyboard shortcuts (PLAN.md 4.4): which key press is which shortcut, and
//! what each one does given what is showing.

use crate::{Message, Ready};
use fortnight_app::top_bar::{self, Tab};
use fortnight_app::views::NavKey;
use fortnight_app::views::day;
use iced::Task;
use iced::keyboard::{self, Key, key::Named};
use iced::widget::operation;

/// A key pressed while no widget captured it (PLAN.md 4.4). Single keys
/// arrive only when no text field has focus, because a focused text input
/// captures them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shortcut {
    Tab(Tab),
    Prev,
    Next,
    Today,
    FocusQuickAdd,
    Search,
    ShortcutsHelp,
    Settings,
    Undo,
    Redo,
    Escape,
    Nav(NavKey),
}

/// The shortcut for a key press that no widget captured (PLAN.md 4.4).
pub(crate) fn shortcut_for(key: Key<&str>, modifiers: keyboard::Modifiers) -> Option<Shortcut> {
    if modifiers.command() {
        return match key {
            Key::Character("1") => Some(Shortcut::Tab(Tab::Today)),
            Key::Character("2") => Some(Shortcut::Tab(Tab::Week)),
            Key::Character("3") => Some(Shortcut::Tab(Tab::Month)),
            Key::Character(",") => Some(Shortcut::Settings),
            Key::Character("n" | "N") => Some(Shortcut::FocusQuickAdd),
            Key::Character("f" | "F") => Some(Shortcut::Search),
            Key::Character("z" | "Z") if modifiers.shift() => Some(Shortcut::Redo),
            Key::Character("z" | "Z") => Some(Shortcut::Undo),
            Key::Character("y" | "Y") if !modifiers.shift() => Some(Shortcut::Redo),
            _ => None,
        };
    }
    if modifiers.alt() || modifiers.control() || modifiers.logo() {
        return None;
    }
    match key {
        Key::Character("1") => Some(Shortcut::Tab(Tab::Today)),
        Key::Character("2") => Some(Shortcut::Tab(Tab::Week)),
        Key::Character("3") => Some(Shortcut::Tab(Tab::Month)),
        Key::Character("t" | "T") => Some(Shortcut::Today),
        Key::Character("n" | "N") => Some(Shortcut::FocusQuickAdd),
        Key::Character("/") => Some(Shortcut::Search),
        Key::Character("?") => Some(Shortcut::ShortcutsHelp),
        Key::Named(Named::ArrowLeft) => Some(Shortcut::Prev),
        Key::Named(Named::ArrowRight) => Some(Shortcut::Next),
        Key::Named(Named::ArrowUp) => Some(Shortcut::Nav(NavKey::Up)),
        Key::Named(Named::ArrowDown) => Some(Shortcut::Nav(NavKey::Down)),
        Key::Named(Named::Space) => Some(Shortcut::Nav(NavKey::Toggle)),
        Key::Named(Named::Enter) => Some(Shortcut::Nav(NavKey::Open)),
        Key::Named(Named::Escape) => Some(Shortcut::Escape),
        _ => None,
    }
}

impl Ready {
    /// Applies a keyboard shortcut. While an overlay or dialog is showing,
    /// only the keys that apply to it run.
    pub(crate) fn shortcut(&mut self, shortcut: Shortcut) -> Task<Message> {
        if self.unlocking {
            if shortcut == Shortcut::Escape {
                self.close_unlock();
            }
            return Task::none();
        }
        if self.shortcuts_open {
            if matches!(shortcut, Shortcut::Escape | Shortcut::ShortcutsHelp) {
                self.shortcuts_open = false;
            }
            return Task::none();
        }
        if self.search_open {
            match shortcut {
                Shortcut::Escape | Shortcut::Search => {
                    self.close_search();
                    return Task::none();
                }
                Shortcut::Nav(NavKey::Up) => {
                    self.local = true;
                    self.search_view.move_selection(-1);
                    return Task::none();
                }
                Shortcut::Nav(NavKey::Down) => {
                    self.local = true;
                    self.search_view.move_selection(1);
                    return Task::none();
                }
                Shortcut::Nav(NavKey::Open) => {
                    let action = self.search_view.activate_selected();
                    return self.search_action(action);
                }
                _ => return Task::none(),
            }
        }
        if self.settings_open {
            if matches!(shortcut, Shortcut::Escape | Shortcut::Settings) {
                self.hide_settings();
            }
            return Task::none();
        }
        if let Some(dialog) = &mut self.dialog {
            if shortcut == Shortcut::Escape {
                // An open calendar closes first, keeping the form.
                if dialog.form.calendar.is_some() {
                    dialog.form.calendar = None;
                } else if dialog.form.planned_calendar.is_some() {
                    dialog.form.planned_calendar = None;
                } else {
                    self.dialog = None;
                }
            }
            return Task::none();
        }
        match shortcut {
            Shortcut::Tab(tab) => self.top_bar(top_bar::Message::Tab(tab)),
            Shortcut::Prev => self.top_bar(top_bar::Message::Prev),
            Shortcut::Next => self.top_bar(top_bar::Message::Next),
            Shortcut::Today => self.top_bar(top_bar::Message::Today),
            // In the Week tab, the new-task dialog for the selected day;
            // elsewhere the Today tab's add-a-task input.
            Shortcut::FocusQuickAdd if self.tab == Tab::Week => self.open_new_task(self.selected),
            Shortcut::FocusQuickAdd => Task::batch([
                self.show(Tab::Today, None),
                operation::focus(day::QUICK_ADD_ID),
            ]),
            Shortcut::Search => self.open_search(),
            Shortcut::ShortcutsHelp => {
                self.date_picker = None;
                self.shortcuts_open = true;
                crate::unfocus()
            }
            Shortcut::Settings => self.open_settings(),
            Shortcut::Undo => {
                self.undo();
                Task::none()
            }
            Shortcut::Redo => {
                self.redo();
                Task::none()
            }
            Shortcut::Escape => {
                if self.date_picker.is_some() {
                    self.date_picker = None;
                    Task::none()
                } else if self.tab == Tab::Today {
                    let action = self.day_view.on_key(NavKey::Close);
                    self.day_action(action)
                } else {
                    Task::none()
                }
            }
            Shortcut::Nav(key) => {
                if self.tab == Tab::Today {
                    let action = self.day_view.on_key(key);
                    self.day_action(action)
                } else {
                    Task::none()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyboard::Modifiers;

    #[test]
    fn shortcuts_follow_plan_4_4() {
        let none = Modifiers::empty();
        let cmd = Modifiers::COMMAND;
        let key = |k: Key<&'static str>, m| shortcut_for(k, m);
        assert_eq!(
            key(Key::Character("2"), none),
            Some(Shortcut::Tab(Tab::Week))
        );
        assert_eq!(
            key(Key::Character("3"), cmd),
            Some(Shortcut::Tab(Tab::Month))
        );
        assert_eq!(key(Key::Character("t"), none), Some(Shortcut::Today));
        assert_eq!(key(Key::Character("n"), cmd), Some(Shortcut::FocusQuickAdd));
        assert_eq!(key(Key::Character("f"), cmd), Some(Shortcut::Search));
        assert_eq!(key(Key::Character("/"), none), Some(Shortcut::Search));
        assert_eq!(
            key(Key::Character("?"), Modifiers::SHIFT),
            Some(Shortcut::ShortcutsHelp)
        );
        assert_eq!(key(Key::Character(","), cmd), Some(Shortcut::Settings));
        assert_eq!(key(Key::Character("z"), cmd), Some(Shortcut::Undo));
        assert_eq!(
            key(Key::Character("z"), cmd | Modifiers::SHIFT),
            Some(Shortcut::Redo)
        );
        assert_eq!(key(Key::Character("y"), cmd), Some(Shortcut::Redo));
        assert_eq!(
            key(Key::Named(Named::ArrowLeft), none),
            Some(Shortcut::Prev)
        );
        assert_eq!(
            key(Key::Named(Named::Space), none),
            Some(Shortcut::Nav(NavKey::Toggle))
        );
        assert_eq!(
            key(Key::Named(Named::Enter), none),
            Some(Shortcut::Nav(NavKey::Open))
        );
        assert_eq!(key(Key::Named(Named::Escape), none), Some(Shortcut::Escape));
        assert_eq!(key(Key::Character("x"), none), None);
        assert_eq!(key(Key::Character("t"), Modifiers::ALT), None);
    }
}
