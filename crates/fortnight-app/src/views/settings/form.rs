//! The editable copy of the non-secret settings, and its validation.

use fortnight_core::domain::{Accent, AppIcon, Appearance, Schedule, Settings, TodayWidth};
use fortnight_core::parse::{format_hours, parse_hours};
use fortnight_core::report::{DEFAULT_PLAN_PROMPT, DEFAULT_SUMMARY_PROMPT, WEEKDAYS};
use jiff::civil::Weekday;

use super::{Endpoint, SettingsView, Which};

/// The editable copy of the non-secret settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Form {
    pub(super) main: Endpoint,
    pub(super) decision: Endpoint,
    pub(super) week_start: Weekday,
    /// Indexed Monday = 0.
    pub(super) working_days: [bool; 7],
    /// Decimal hours as typed, e.g. `5.5`.
    pub(super) focus_hours: String,
    pub(super) appearance: Appearance,
    pub(super) app_icon: AppIcon,
    pub(super) accent: Accent,
    pub(super) today_width: TodayWidth,
    /// The prompts as shown and edited.
    pub(super) summary_prompt: String,
    pub(super) plan_prompt: String,
    /// The summary prompt as first shown, and the stored fields it came
    /// from, so an untouched prompt saves back exactly as it was.
    pub(super) summary_shown: String,
    pub(super) summary_stored: (String, String),
}

impl Default for Form {
    fn default() -> Self {
        Form::from_settings(&Settings::default())
    }
}

impl Form {
    pub(super) fn from_settings(s: &Settings) -> Self {
        let sched = &s.schedule;
        let mut summary_shown = match s.summary_prompt.trim() {
            "" => DEFAULT_SUMMARY_PROMPT.to_string(),
            custom => custom.to_string(),
        };
        let extra = s.summary_instructions.trim();
        if !extra.is_empty() {
            summary_shown.push_str("\n\n");
            summary_shown.push_str(extra);
        }
        Form {
            main: Endpoint::from_llm(&s.llm),
            decision: Endpoint::from_llm(&s.decision_llm),
            week_start: sched.week_start,
            working_days: WEEKDAYS.map(|d| sched.working_days.contains(&d)),
            focus_hours: format_hours(s.focus_minutes_per_day),
            appearance: s.appearance,
            app_icon: s.app_icon,
            accent: s.accent,
            today_width: s.today_width,
            summary_prompt: summary_shown.clone(),
            plan_prompt: match s.plan_prompt.trim() {
                "" => DEFAULT_PLAN_PROMPT.to_string(),
                custom => custom.to_string(),
            },
            summary_shown,
            summary_stored: (s.summary_prompt.clone(), s.summary_instructions.clone()),
        }
    }

    /// The settings this form describes, or `None` if a time or the focus
    /// hours are invalid or no working day is ticked.
    pub(super) fn to_settings(&self) -> Option<Settings> {
        if !self.working_days.contains(&true) {
            return None;
        }
        let (summary_prompt, summary_instructions) = if self.summary_prompt == self.summary_shown {
            self.summary_stored.clone()
        } else {
            (
                stored_prompt(&self.summary_prompt, DEFAULT_SUMMARY_PROMPT),
                String::new(),
            )
        };
        Some(Settings {
            llm: self.main.llm(),
            decision_llm: self.decision.llm(),
            schedule: Schedule {
                week_start: self.week_start,
                working_days: WEEKDAYS
                    .iter()
                    .zip(self.working_days)
                    .filter(|(_, on)| *on)
                    .map(|(d, _)| *d)
                    .collect(),
            },
            summary_instructions,
            focus_minutes_per_day: parse_hours(&self.focus_hours)?,
            appearance: self.appearance,
            app_icon: self.app_icon,
            accent: self.accent,
            today_width: self.today_width,
            summary_prompt,
            plan_prompt: stored_prompt(&self.plan_prompt, DEFAULT_PLAN_PROMPT),
        })
    }

    pub(super) fn endpoint(&self, which: Which) -> &Endpoint {
        match which {
            Which::Main => &self.main,
            Which::Decision => &self.decision,
        }
    }

    pub(super) fn endpoint_mut(&mut self, which: Which) -> &mut Endpoint {
        match which {
            Which::Main => &mut self.main,
            Which::Decision => &mut self.decision,
        }
    }
}

/// A prompt as stored: empty when it is the built-in one.
pub(super) fn stored_prompt(text: &str, default: &str) -> String {
    let text = text.trim();
    if text == default.trim() {
        String::new()
    } else {
        text.to_string()
    }
}

/// The form against the loaded settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FormStatus {
    /// Nothing to save.
    Clean,
    /// Edited, but a field is invalid.
    Invalid,
    /// Edited and valid: Save saves them.
    Savable,
}

impl SettingsView {
    /// The edits: `None` when there is nothing to save, `Some(None)` when
    /// a field is invalid, else the settings Save would save.
    pub(super) fn edits(&self) -> Option<Option<Settings>> {
        // An untouched form saves nothing even when it doesn't round-trip
        // exactly (older settings: keyless, untrimmed prompts).
        if self.form == self.baseline {
            return None;
        }
        match self.form.to_settings() {
            // Edits that parse back to the same settings ("8" → "8.0").
            Some(s) if s == self.loaded => None,
            parsed => Some(parsed),
        }
    }

    pub(super) fn status(&self) -> FormStatus {
        match self.edits() {
            None => FormStatus::Clean,
            Some(None) => FormStatus::Invalid,
            Some(Some(_)) => FormStatus::Savable,
        }
    }

    /// The settings the form would save, if they differ from the loaded
    /// ones and every field is valid.
    pub(super) fn savable(&self) -> Option<Settings> {
        self.edits().flatten()
    }

    #[cfg(test)]
    pub(super) fn is_dirty(&self) -> bool {
        !matches!(self.status(), FormStatus::Clean)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::super::*;
    use fortnight_core::domain::{DEFAULT_FOCUS_MINUTES, Protocol};

    #[test]
    fn save_emits_full_settings() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::BaseUrl(
            Which::Main,
            " https://api.openai.com/v1 ".into(),
        ));
        v.update(Message::Model(Which::Main, "gpt-4o".into()));
        v.update(Message::WeekStart(Weekday::Monday));
        v.update(Message::WorkingDay(4, false));
        v.update(Message::WorkingDay(5, true));
        v.update(Message::ResetPrompt(PromptKind::Plan));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.llm.base_url, "https://api.openai.com/v1");
        assert_eq!(s.llm.model, "gpt-4o");
        assert_eq!(s.schedule.week_start, Weekday::Monday);
        assert_eq!(
            s.schedule.working_days,
            vec![
                Weekday::Monday,
                Weekday::Tuesday,
                Weekday::Wednesday,
                Weekday::Thursday,
                Weekday::Saturday
            ]
        );
        assert_eq!(s.summary_instructions, "");
        assert_eq!(s.plan_prompt, "", "the built-in prompt is stored as empty");
    }

    #[test]
    fn accent_choice_saves() {
        let mut v = loaded(KeyState::NoKey);
        assert_eq!(v.form.accent, Accent::Orange);
        v.update(Message::Accent(Accent::Purple));
        assert!(v.is_dirty());
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.accent, Accent::Purple);
    }

    #[test]
    fn today_width_choice_saves() {
        let mut v = loaded(KeyState::NoKey);
        assert_eq!(v.form.today_width, TodayWidth::Standard);
        v.update(Message::TodayWidth(TodayWidth::Wide));
        assert!(v.is_dirty());
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.today_width, TodayWidth::Wide);
    }

    #[test]
    fn appearance_choice_saves() {
        let mut v = loaded(KeyState::NoKey);
        assert_eq!(v.form.appearance, Appearance::System);
        v.update(Message::Appearance(Appearance::Dark));
        assert!(v.is_dirty());
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.appearance, Appearance::Dark);
    }

    #[test]
    fn app_icon_choice_saves() {
        let mut v = loaded(KeyState::NoKey);
        assert_eq!(v.form.app_icon, AppIcon::System);
        v.update(Message::AppIcon(AppIcon::Dark));
        assert!(v.is_dirty());
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.app_icon, AppIcon::Dark);
    }

    #[test]
    fn save_does_nothing_when_unchanged() {
        let mut v = loaded(KeyState::NoKey);
        assert!(matches!(v.update(Message::Save), Action::None));
    }

    #[test]
    fn prompts_edit_reset_and_save() {
        let mut v = loaded(KeyState::NoKey);
        assert_eq!(v.form.summary_prompt, DEFAULT_SUMMARY_PROMPT);
        v.update(Message::Prompt(
            PromptKind::Summary,
            text_editor::Action::Move(text_editor::Motion::DocumentStart),
        ));
        v.update(Message::Prompt(
            PromptKind::Summary,
            text_editor::Action::Edit(text_editor::Edit::Paste(std::sync::Arc::new(
                "Be brief. ".to_string(),
            ))),
        ));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert!(s.summary_prompt.starts_with("Be brief. You write"));
        assert_eq!(s.plan_prompt, "");
        v.update(Message::ResetPrompt(PromptKind::Summary));
        assert!(
            matches!(v.update(Message::Save), Action::None),
            "back to unchanged"
        );
    }

    #[test]
    fn round2_summary_instructions_are_folded_into_the_prompt() {
        let s = Settings {
            summary_instructions: "Short.".into(),
            ..Settings::default()
        };
        let mut v = SettingsView::new();
        v.load(data(s.clone(), KeyState::NoKey));
        assert!(v.form.summary_prompt.ends_with("\n\nShort."));
        assert!(!v.is_dirty());
        assert!(matches!(v.update(Message::Save), Action::None));
        // Any other change keeps them as they are.
        v.update(Message::Model(Which::Main, "m".into()));
        let Action::Save(saved) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(saved.summary_instructions, "Short.");
        assert_eq!(saved.summary_prompt, "");
        // Editing the prompt folds them in for good.
        v.update(Message::Prompt(
            PromptKind::Summary,
            text_editor::Action::Edit(text_editor::Edit::Insert('!')),
        ));
        let Action::Save(saved) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(saved.summary_instructions, "");
        assert!(saved.summary_prompt.contains("Short."));
    }

    #[test]
    fn protocol_and_focus_hours_round_trip_through_save() {
        let mut v = loaded(KeyState::NoKey);
        assert_eq!(v.form.focus_hours, "8");
        v.update(Message::Provider(Which::Main, Provider::Anthropic));
        v.update(Message::Model(Which::Main, "claude-x".into()));
        v.update(Message::FocusHours(" 5.5 ".into()));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.llm.protocol, Protocol::Anthropic);
        assert_eq!(s.focus_minutes_per_day, 330);
        // Saved settings come back from main.rs: the form matches them and
        // there is nothing left to save.
        v.load(data(s.clone(), KeyState::NoKey));
        assert_eq!(v.form.main.provider, Provider::Anthropic);
        assert_eq!(v.form.focus_hours, "5.5");
        assert!(!v.is_dirty());
        assert!(matches!(v.update(Message::Save), Action::None));
    }

    #[test]
    fn round1_settings_load_with_defaults_and_save_unchanged() {
        // Round 1 settings JSON has no protocol or focus minutes; core's
        // serde defaults fill them in (tested in `domain`), which gives this
        // value.
        let mut round1 = Settings::default();
        round1.llm.base_url = "http://localhost:11434/v1".into();
        round1.llm.model = "llama3.2".into();
        round1.summary_instructions = "Short.".into();
        let mut v = SettingsView::new();
        v.load(data(round1.clone(), KeyState::NoKey));
        // On Ollama's port, so it shows as Ollama.
        assert_eq!(v.form.main.provider, Provider::Ollama);
        assert_eq!(v.form.focus_hours, "8");
        assert!(!v.is_dirty());
        v.update(Message::FocusHours("8.5".into()));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        // Saved as Ollama: the same endpoint, now marked keyless.
        assert_eq!(s.llm.base_url, round1.llm.base_url);
        assert_eq!(s.llm.model, round1.llm.model);
        assert_eq!(s.llm.protocol, round1.llm.protocol);
        assert!(s.llm.keyless);
        assert_eq!(s.schedule, round1.schedule);
        assert_eq!(s.focus_minutes_per_day, DEFAULT_FOCUS_MINUTES + 30);
        assert_eq!(s.summary_instructions, "Short.");
    }

    #[test]
    fn invalid_focus_hours_block_save() {
        let mut v = loaded(KeyState::NoKey);
        for bad in ["", "0", "abc", "25", "-3", "5h", "1.2.3"] {
            v.update(Message::FocusHours(bad.into()));
            assert!(matches!(v.update(Message::Save), Action::None), "{bad}");
            assert!(v.is_dirty());
        }
        v.update(Message::FocusHours("7,5".into()));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.focus_minutes_per_day, 450);
    }

    #[test]
    fn edits_that_parse_to_the_loaded_settings_are_not_unsaved() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::FocusHours("8.0".into()));
        assert!(!v.is_dirty());
        assert!(v.savable().is_none());
        assert!(matches!(v.update(Message::Save), Action::None));
        v.update(Message::FocusHours("8.5".into()));
        assert!(v.is_dirty());
        assert!(v.savable().is_some());
        // Invalid counts as unsaved (the header asks to fix it).
        v.update(Message::FocusHours("x".into()));
        assert!(v.is_dirty());
        assert!(v.savable().is_none());
    }

    #[test]
    fn no_working_days_cannot_be_saved() {
        let mut v = loaded(KeyState::NoKey);
        v.update(Message::Model(Which::Main, "m".into()));
        for i in 0..7 {
            v.update(Message::WorkingDay(i, false));
        }
        assert!(v.form.to_settings().is_none());
        assert!(matches!(v.update(Message::Save), Action::None));
        v.update(Message::WorkingDay(2, true));
        let Action::Save(s) = v.update(Message::Save) else {
            panic!("expected Save");
        };
        assert_eq!(s.schedule.working_days, vec![Weekday::Wednesday]);
    }
}
