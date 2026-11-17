//! The Review view: the week behind. Facts beside the summary; edit, copy,
//! regenerate; a week switcher. Owner: agent G.
//!
//! Layout: a header (title and `← Week 47 · Nov 16–22 →`), then two columns:
//! "The facts" (Markdown in a card) and "Summary" (state-dependent: generating,
//! blocked, empty, failed, ready, or editing).
//!
//! The view is wider than [`theme::CONTENT_WIDTH`] (see [`REVIEW_WIDTH`]) so
//! the two columns each get a readable line length.

use fortnight_core::domain::{Report, ReportId, ReportStatus, WeekRef};
use iced::widget::{
    Column, button, column, container, markdown, row, scrollable, space, text, text_editor,
};
use iced::{Center, Element, Fill, FillPortion, Font, Padding, border, color};

use crate::theme;

/// Max width of this view: two columns need more room than one.
const REVIEW_WIDTH: f32 = theme::CONTENT_WIDTH * 1.5;

#[derive(Debug, Clone)]
pub enum Message {
    PrevWeek,
    NextWeek,
    Generate,
    StartEdit,
    EditAction(text_editor::Action),
    SaveEdit,
    CancelEdit,
    Copy,
    /// A Markdown link was clicked; ignored on purpose.
    LinkClicked(markdown::Uri),
}

/// What `main.rs` must do. Contract: do not change without the orchestrator.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Show another week; `main.rs` responds by calling `load`.
    ShowWeek(WeekRef),
    /// Generate (or regenerate) the summary for this week.
    Generate(WeekRef),
    /// Save the user's edit of a report.
    SaveEdit(ReportId, String),
    /// Copy this text to the clipboard.
    Copy(String),
}

/// Everything the view shows, supplied by `main.rs`.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewData {
    pub week: WeekRef,
    /// e.g. "Week 46 · Nov 9–15".
    pub week_label: String,
    /// The deterministic facts, as Markdown.
    pub facts_md: String,
    /// The latest summary for the week, if any (may be `Failed`).
    pub report: Option<Report>,
    /// A summary for this week is being generated right now.
    pub generating: bool,
    /// Why generating is not possible right now, e.g. "Unlock to generate"
    /// or "Set up an endpoint in Settings". `None` when it is possible.
    pub blocked: Option<String>,
}

#[derive(Debug, Default)]
pub struct ReviewView {
    /// What `load` last received; `None` before the first load.
    data: Option<ReviewData>,
    /// `data.facts_md`, parsed.
    facts: markdown::Content,
    /// `data.report.text()`, parsed (empty when there is none).
    summary: markdown::Content,
    /// The open editor, if the user is editing the summary.
    editor: Option<text_editor::Content>,
}

impl ReviewView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces what the view shows. `main.rs` calls this after EVERY
    /// message (and on the minute tick), so it must be cheap: if `data`
    /// equals what the view already has, return early. It must keep
    /// in-progress UI state (an open editor, typed text) unless the thing it
    /// belongs to changed (a different week or report id).
    pub fn load(&mut self, data: ReviewData) {
        if self.data.as_ref() == Some(&data) {
            return;
        }
        let old = self.data.take();
        let old_week = old.as_ref().map(|d| d.week);
        let old_id = old.as_ref().and_then(|d| d.report.as_ref().map(|r| r.id));
        let old_facts = old.as_ref().map(|d| d.facts_md.as_str());
        let old_text = old.as_ref().and_then(|d| d.report.as_ref()?.text());

        if old_week != Some(data.week) || old_id != data.report.as_ref().map(|r| r.id) {
            self.editor = None;
        }
        if old_facts != Some(data.facts_md.as_str()) {
            self.facts = markdown::Content::parse(&data.facts_md);
        }
        let new_text = data.report.as_ref().and_then(Report::text);
        if old.is_none() || old_text != new_text {
            self.summary = markdown::Content::parse(new_text.unwrap_or_default());
        }
        self.data = Some(data);
    }

    /// The text in the open editor, if editing.
    pub fn editor_text(&self) -> Option<String> {
        self.editor.as_ref().map(text_editor::Content::text)
    }

    pub fn update(&mut self, message: Message) -> Action {
        let Some(data) = &self.data else {
            return Action::None;
        };
        match message {
            Message::PrevWeek => Action::ShowWeek(data.week.prev()),
            Message::NextWeek => Action::ShowWeek(data.week.next()),
            Message::Generate => {
                if data.blocked.is_some() || data.generating {
                    Action::None
                } else {
                    self.editor = None;
                    Action::Generate(data.week)
                }
            }
            Message::StartEdit => {
                if let Some(text) = data.report.as_ref().and_then(Report::text) {
                    self.editor = Some(text_editor::Content::with_text(text));
                }
                Action::None
            }
            Message::EditAction(action) => {
                if let Some(editor) = &mut self.editor {
                    editor.perform(action);
                }
                Action::None
            }
            Message::SaveEdit => match (self.editor.take(), &data.report) {
                (Some(editor), Some(report)) => Action::SaveEdit(report.id, editor.text()),
                _ => Action::None,
            },
            Message::CancelEdit => {
                self.editor = None;
                Action::None
            }
            Message::Copy => match data.report.as_ref().and_then(Report::text) {
                Some(text) => Action::Copy(text.to_string()),
                None => Action::None,
            },
            Message::LinkClicked(_) => Action::None,
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let Some(data) = &self.data else {
            return container(text("Loading…").style(theme::muted))
                .padding(theme::SPACING * 2.0)
                .into();
        };

        let header = row![
            theme::title("Review"),
            space::horizontal(),
            button(text("←").size(theme::TEXT_HEADING))
                .on_press(Message::PrevWeek)
                .style(theme::text_button),
            text(&data.week_label).size(theme::TEXT_BODY),
            button(text("→").size(theme::TEXT_HEADING))
                .on_press(Message::NextWeek)
                .style(theme::text_button),
        ]
        .spacing(theme::SPACING / 2.0)
        .align_y(Center);

        let facts = column![
            theme::section_heading("The facts"),
            container(md(&self.facts))
                .padding(theme::SPACING * 1.5)
                .width(Fill)
                .style(theme::card),
        ]
        .spacing(theme::SPACING / 2.0)
        .width(FillPortion(2));

        let summary = column![theme::section_heading("Summary"), self.summary_view(data)]
            .spacing(theme::SPACING / 2.0)
            .width(FillPortion(3));

        let body = column![header, row![facts, summary].spacing(theme::SPACING * 2.0)]
            .spacing(theme::SPACING * 2.0)
            .padding(theme::SPACING * 2.0)
            .max_width(REVIEW_WIDTH);

        scrollable(container(body).center_x(Fill)).into()
    }

    fn summary_view<'a>(&'a self, data: &'a ReviewData) -> Element<'a, Message> {
        let generate_label = |label| text(label).size(theme::TEXT_BODY);
        let blocked_hint =
            |reason: &'a str| text(reason).size(theme::TEXT_SMALL).style(theme::muted);

        if data.generating {
            return text("Generating…").style(theme::muted).into();
        }
        let Some(report) = &data.report else {
            let mut col = Column::new().spacing(theme::SPACING);
            let mut generate = button(generate_label("Generate"))
                .padding([8, 16])
                .style(theme::primary_button);
            match &data.blocked {
                Some(reason) => {
                    col = col.push(blocked_hint(reason));
                }
                None => {
                    col = col.push(text("No summary for this week yet").style(theme::muted));
                    generate = generate.on_press(Message::Generate);
                }
            }
            return col.push(generate).into();
        };

        if report.status == ReportStatus::Failed {
            let error = report.error.as_deref().unwrap_or("Generation failed.");
            let mut retry = button(generate_label("Retry"))
                .padding([8, 16])
                .style(theme::primary_button);
            if data.blocked.is_none() {
                retry = retry.on_press(Message::Generate);
            }
            let mut col = column![text(error).style(theme::danger), retry].spacing(theme::SPACING);
            if let Some(reason) = &data.blocked {
                col = col.push(blocked_hint(reason));
            }
            return col.into();
        }

        if let Some(editor) = &self.editor {
            let editor = text_editor(editor)
                .on_action(Message::EditAction)
                .min_height(360.0)
                .padding(theme::SPACING)
                .size(theme::TEXT_BODY)
                .style(theme::editor);
            let buttons = row![
                button(text("Save").size(theme::TEXT_BODY))
                    .on_press(Message::SaveEdit)
                    .padding([8, 16])
                    .style(theme::primary_button),
                button(text("Cancel").size(theme::TEXT_BODY))
                    .on_press(Message::CancelEdit)
                    .padding([8, 16])
                    .style(theme::secondary_button),
            ]
            .spacing(theme::SPACING / 2.0);
            return column![editor, buttons].spacing(theme::SPACING).into();
        }

        let mut regenerate = button(text("Regenerate").size(theme::TEXT_BODY))
            .padding([8, 16])
            .style(theme::secondary_button);
        if data.blocked.is_none() {
            regenerate = regenerate.on_press(Message::Generate);
        }
        let buttons = row![
            button(text("Edit").size(theme::TEXT_BODY))
                .on_press(Message::StartEdit)
                .padding([8, 16])
                .style(theme::secondary_button),
            button(text("Copy").size(theme::TEXT_BODY))
                .on_press(Message::Copy)
                .padding([8, 16])
                .style(theme::secondary_button),
            regenerate,
        ]
        .spacing(theme::SPACING / 2.0)
        .align_y(Center);

        let mut col = column![
            container(md(&self.summary))
                .padding([theme::SPACING, 0.0])
                .width(Fill),
            buttons,
        ]
        .spacing(theme::SPACING);
        if let Some(reason) = &data.blocked {
            col = col.push(blocked_hint(reason));
        }
        col.push(
            text(metadata(report))
                .size(theme::TEXT_SMALL)
                .style(theme::muted),
        )
        .into()
    }
}

/// Renders parsed Markdown with the app's sizes and accent; links are
/// reported as `LinkClicked` and ignored.
fn md(content: &markdown::Content) -> Element<'_, Message> {
    let style = markdown::Style {
        font: Font::default(),
        inline_code_highlight: markdown::Highlight {
            background: color!(0x78716c, 0.2).into(),
            border: border::rounded(4),
        },
        inline_code_padding: Padding::from([0, 2]),
        inline_code_color: theme::ACCENT,
        inline_code_font: Font::MONOSPACE,
        code_block_font: Font::MONOSPACE,
        link_color: theme::ACCENT,
    };
    let settings = markdown::Settings::with_text_size(theme::TEXT_BODY, style);
    markdown::view(content.items(), settings).map(Message::LinkClicked)
}

/// "Generated by gpt-4o-mini · Nov 20, 16:02 (edited)".
fn metadata(report: &Report) -> String {
    let when = report
        .created_at
        .to_zoned_datetime(jiff::tz::TimeZone::system())
        .strftime("%b %-d, %H:%M")
        .to_string();
    let mut s = match &report.model {
        Some(model) => format!("Generated by {model} · {when}"),
        None => format!("Generated {when}"),
    };
    if report.edited_md.is_some() {
        s.push_str(" · edited");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use fortnight_core::domain::ReportKind;
    use jiff::Timestamp;
    use jiff::civil::date;

    fn week() -> WeekRef {
        WeekRef::new(date(2026, 11, 16))
    }

    fn report(id: ReportId) -> Report {
        Report {
            id,
            kind: ReportKind::Summary,
            week_start: week().start,
            status: ReportStatus::Ready,
            digest_json: "{}".into(),
            content_md: Some("## Highlights\n- Shipped it".into()),
            edited_md: None,
            model: Some("test-model".into()),
            endpoint_host: None,
            prompt_version: 1,
            error: None,
            created_at: Timestamp::UNIX_EPOCH,
        }
    }

    fn data(report: Option<Report>) -> ReviewData {
        ReviewData {
            week: week(),
            week_label: "Week 47 · Nov 16–22".into(),
            facts_md: "- Done: one thing".into(),
            report,
            generating: false,
            blocked: None,
        }
    }

    fn view_editing(id: ReportId) -> ReviewView {
        let mut v = ReviewView::new();
        v.load(data(Some(report(id))));
        assert_eq!(v.update(Message::StartEdit), Action::None);
        v.update(Message::EditAction(text_editor::Action::Move(
            text_editor::Motion::DocumentEnd,
        )));
        v.update(Message::EditAction(text_editor::Action::Edit(
            text_editor::Edit::Insert('!'),
        )));
        v
    }

    #[test]
    fn load_with_equal_data_keeps_editor_text() {
        let mut v = view_editing(1);
        let typed = v.editor_text().unwrap();
        assert!(typed.ends_with("it!"), "{typed:?}");
        v.load(data(Some(report(1))));
        assert_eq!(v.editor_text().as_deref(), Some(typed.as_str()));
    }

    #[test]
    fn load_with_other_report_id_closes_editor() {
        let mut v = view_editing(1);
        v.load(data(Some(report(2))));
        assert_eq!(v.editor_text(), None);
    }

    #[test]
    fn load_with_other_week_closes_editor() {
        let mut v = view_editing(1);
        let mut d = data(Some(report(1)));
        d.week = week().next();
        v.load(d);
        assert_eq!(v.editor_text(), None);
    }

    #[test]
    fn prev_and_next_show_adjacent_weeks() {
        let mut v = ReviewView::new();
        v.load(data(None));
        assert_eq!(
            v.update(Message::PrevWeek),
            Action::ShowWeek(WeekRef::new(date(2026, 11, 9)))
        );
        assert_eq!(
            v.update(Message::NextWeek),
            Action::ShowWeek(WeekRef::new(date(2026, 11, 23)))
        );
    }

    #[test]
    fn generate_asks_for_this_week_unless_blocked() {
        let mut v = ReviewView::new();
        v.load(data(None));
        assert_eq!(v.update(Message::Generate), Action::Generate(week()));

        let mut blocked = data(None);
        blocked.blocked = Some("Unlock to generate".into());
        v.load(blocked);
        assert_eq!(v.update(Message::Generate), Action::None);
    }

    #[test]
    fn copy_prefers_the_edit() {
        let mut v = ReviewView::new();
        v.load(data(Some(report(1))));
        assert_eq!(
            v.update(Message::Copy),
            Action::Copy("## Highlights\n- Shipped it".into())
        );

        let mut edited = report(1);
        edited.edited_md = Some("my edit".into());
        v.load(data(Some(edited)));
        assert_eq!(v.update(Message::Copy), Action::Copy("my edit".into()));
    }

    #[test]
    fn save_sends_the_edited_text_and_closes_the_editor() {
        let mut v = view_editing(7);
        assert_eq!(
            v.update(Message::SaveEdit),
            Action::SaveEdit(7, "## Highlights\n- Shipped it!".into())
        );
        assert_eq!(v.editor_text(), None);
    }

    #[test]
    fn cancel_discards_the_edit() {
        let mut v = view_editing(7);
        assert_eq!(v.update(Message::CancelEdit), Action::None);
        assert_eq!(v.editor_text(), None);
        assert_eq!(v.update(Message::SaveEdit), Action::None);
    }
}
