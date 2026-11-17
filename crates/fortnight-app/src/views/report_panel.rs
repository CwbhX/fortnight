//! The report panel: Advice or Summary for one week, embedded by the Week
//! tab. Owner: agent L (round 2).
//!
//! STEP 1 PLACEHOLDER: round 1's Review view behind the round 2 contract.
//! It shows the first slot only (facts beside the latest version, edit,
//! copy, regenerate) with a plain kind switch. Agent L adds streamed text,
//! versions, export and "what will be sent".

use fortnight_core::domain::{Report, ReportId, ReportKind, ReportStatus, WeekRef};
use fortnight_core::export::ExportFormat;
use iced::widget::{Column, button, column, container, markdown, row, text, text_editor};
use iced::{Center, Element, Fill, FillPortion, Font, Padding, border, color};

use crate::theme;

#[derive(Debug, Clone)]
pub enum Message {
    /// Show the slot of this kind.
    ShowKind(ReportKind),
    Generate,
    StartEdit,
    EditAction(text_editor::Action),
    SaveEdit,
    CancelEdit,
    Copy,
    /// Export the shown version as Markdown.
    Export,
    /// A Markdown link was clicked; ignored on purpose.
    LinkClicked(markdown::Uri),
}

/// What `main.rs` must do. FROZEN contract (round 2).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Generate (or regenerate) this kind of report for this week. Inserts
    /// a new version; earlier ones are kept.
    Generate(ReportKind, WeekRef),
    /// Save the user's edit of a report version.
    SaveEdit(ReportId, String),
    /// Copy this text to the clipboard.
    Copy(String),
    /// Export this report version to a file (`main.rs` asks where).
    Export(ReportId, ExportFormat),
}

/// One kind of report for the week. FROZEN contract (round 2).
#[derive(Debug, Clone, PartialEq)]
pub struct ReportSlot {
    pub kind: ReportKind,
    /// The deterministic facts, as Markdown.
    pub facts_md: String,
    /// Exactly what generating now would send (system prompt and user
    /// message), for "what will be sent". Private tasks are already out.
    pub preview: String,
    /// Every version, newest first (may include `Failed` ones). Each keeps
    /// the `digest_json` it was built from.
    pub versions: Vec<Report>,
    /// `Some(text so far)` while a version is being generated (streamed);
    /// `None` otherwise.
    pub streaming: Option<String>,
    /// Why generating is not possible right now, e.g. "Unlock the API key
    /// to generate." `None` when it is possible.
    pub blocked: Option<String>,
}

/// Everything the panel shows, supplied by `main.rs`. FROZEN contract
/// (round 2).
#[derive(Debug, Clone, PartialEq)]
pub struct ReportPanelData {
    pub week: WeekRef,
    /// e.g. "Week 46 · Nov 9–15".
    pub week_label: String,
    /// One or two slots in display order; the first is shown by default.
    /// A past week has `[Summary]`; the current week `[Plan]`, or
    /// `[Summary, Plan]` once its summary is due; a future week `[Plan]`.
    pub slots: Vec<ReportSlot>,
}

#[derive(Debug, Default)]
pub struct ReportPanel {
    /// What `load` last received; `None` before the first load.
    data: Option<ReportPanelData>,
    /// The kind shown; `None` means the first slot.
    kind: Option<ReportKind>,
    /// `data.facts_md`, parsed.
    facts: markdown::Content,
    /// `data.report.text()`, parsed (empty when there is none).
    summary: markdown::Content,
    /// The open editor, if the user is editing the summary.
    editor: Option<text_editor::Content>,
}

impl ReportPanel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces what the panel shows. `main.rs` calls this after EVERY
    /// message (and on the minute tick and every streamed piece of text), so
    /// it must be cheap: if `data` equals what the panel already has, return
    /// early. It must keep in-progress UI state (an open editor, typed text,
    /// the version shown) unless the thing it belongs to changed (a
    /// different week, or a new version arrived).
    pub fn load(&mut self, data: ReportPanelData) {
        if self.data.as_ref() == Some(&data) {
            return;
        }
        let old = self.data.take();
        if old.as_ref().map(|d| d.week) != Some(data.week) {
            self.kind = None;
        }
        let kind = self.kind;
        let pick = |d: &ReportPanelData| -> Option<ReportSlot> {
            kind.and_then(|k| d.slots.iter().find(|s| s.kind == k))
                .or(d.slots.first())
                .cloned()
        };
        let old_slot = old.as_ref().and_then(pick);
        let new_slot = pick(&data);
        let old_week = old.as_ref().map(|d| d.week);
        let old_id = old_slot
            .as_ref()
            .and_then(|s| s.versions.first().map(|r| r.id));
        let new_id = new_slot
            .as_ref()
            .and_then(|s| s.versions.first().map(|r| r.id));
        let old_facts = old_slot.as_ref().map(|s| s.facts_md.clone());
        let old_text = old_slot.as_ref().and_then(slot_text);
        let new_text = new_slot.as_ref().and_then(slot_text);

        if old_week != Some(data.week) || old_id != new_id {
            self.editor = None;
        }
        let new_facts = new_slot.as_ref().map(|s| s.facts_md.clone());
        if old_facts != new_facts {
            self.facts = markdown::Content::parse(new_facts.as_deref().unwrap_or_default());
        }
        if old.is_none() || old_text != new_text {
            self.summary = markdown::Content::parse(new_text.as_deref().unwrap_or_default());
        }
        self.data = Some(data);
    }

    fn slot(&self) -> Option<&ReportSlot> {
        let data = self.data.as_ref()?;
        self.kind
            .and_then(|k| data.slots.iter().find(|s| s.kind == k))
            .or(data.slots.first())
    }

    /// The text in the open editor, if editing.
    pub fn editor_text(&self) -> Option<String> {
        self.editor.as_ref().map(text_editor::Content::text)
    }

    pub fn update(&mut self, message: Message) -> Action {
        if let Message::ShowKind(kind) = message {
            self.kind = Some(kind);
            self.editor = None;
            // Reparse for the newly shown slot.
            if let Some(data) = self.data.take() {
                let slot = self.slot_in(&data).cloned();
                self.facts = markdown::Content::parse(
                    slot.as_ref()
                        .map(|s| s.facts_md.as_str())
                        .unwrap_or_default(),
                );
                self.summary = markdown::Content::parse(
                    slot.as_ref()
                        .and_then(slot_text)
                        .as_deref()
                        .unwrap_or_default(),
                );
                self.data = Some(data);
            }
            return Action::None;
        }
        let (Some(data), Some(slot)) = (&self.data, self.slot()) else {
            return Action::None;
        };
        let report = slot.versions.first();
        match message {
            Message::ShowKind(_) => Action::None,
            Message::Generate => {
                if slot.blocked.is_some() || slot.streaming.is_some() {
                    Action::None
                } else {
                    let action = Action::Generate(slot.kind, data.week);
                    self.editor = None;
                    action
                }
            }
            Message::StartEdit => {
                if let Some(text) = report.and_then(Report::text) {
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
            Message::SaveEdit => match report.map(|r| r.id) {
                Some(id) => match self.editor.take() {
                    Some(editor) => Action::SaveEdit(id, editor.text()),
                    None => Action::None,
                },
                None => Action::None,
            },
            Message::CancelEdit => {
                self.editor = None;
                Action::None
            }
            Message::Copy => match report.and_then(Report::text) {
                Some(text) => Action::Copy(text.to_string()),
                None => Action::None,
            },
            Message::Export => match report {
                Some(r) => Action::Export(r.id, ExportFormat::Markdown),
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

        let Some(slot) = self.slot() else {
            return Column::new().into();
        };
        let mut header = row![].spacing(theme::SPACING / 2.0).align_y(Center);
        for s in &data.slots {
            let label = kind_label(s.kind);
            header = header.push(
                button(text(label).size(theme::TEXT_BODY))
                    .padding(theme::tab_padding())
                    .style(theme::tab_button(s.kind == slot.kind))
                    .on_press(Message::ShowKind(s.kind)),
            );
        }

        let facts = column![
            theme::section_heading("The facts"),
            container(md(&self.facts))
                .padding(theme::SPACING * 1.5)
                .width(Fill)
                .style(theme::card),
        ]
        .spacing(theme::SPACING / 2.0)
        .width(FillPortion(2));

        let summary = column![
            theme::section_heading(kind_label(slot.kind)),
            self.summary_view(slot)
        ]
        .spacing(theme::SPACING / 2.0)
        .width(FillPortion(3));

        column![header, row![facts, summary].spacing(theme::SPACING * 2.0)]
            .spacing(theme::SPACING * 1.5)
            .into()
    }

    fn slot_in<'a>(&self, data: &'a ReportPanelData) -> Option<&'a ReportSlot> {
        self.kind
            .and_then(|k| data.slots.iter().find(|s| s.kind == k))
            .or(data.slots.first())
    }

    fn summary_view<'a>(&'a self, data: &'a ReportSlot) -> Element<'a, Message> {
        let generate_label = |label| text(label).size(theme::TEXT_BODY);
        let blocked_hint =
            |reason: &'a str| text(reason).size(theme::TEXT_SMALL).style(theme::muted);

        if let Some(so_far) = &data.streaming {
            let shown = if so_far.is_empty() {
                "Generating…"
            } else {
                so_far
            };
            return text(shown).style(theme::muted).into();
        }
        let Some(report) = data.versions.first() else {
            let mut col = Column::new().spacing(theme::SPACING);
            let mut generate = button(generate_label("Generate"))
                .padding([8, 16])
                .style(theme::primary_button);
            match &data.blocked {
                Some(reason) => {
                    col = col.push(blocked_hint(reason));
                }
                None => {
                    col = col.push(text("Nothing generated for this week yet").style(theme::muted));
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
            button(text("Export").size(theme::TEXT_BODY))
                .on_press(Message::Export)
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

/// The text of the newest version, if any.
fn slot_text(slot: &ReportSlot) -> Option<String> {
    slot.versions
        .first()
        .and_then(Report::text)
        .map(str::to_owned)
}

fn kind_label(kind: ReportKind) -> &'static str {
    match kind {
        ReportKind::Summary => "Summary",
        ReportKind::Plan => "Advice",
    }
}

/// Renders parsed Markdown with the app's sizes and accent; links are
/// reported as `LinkClicked` and ignored.
fn md(content: &markdown::Content) -> Element<'_, Message> {
    let style = markdown::Style {
        font: theme::FONT,
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

    fn data(report: Option<Report>) -> ReportPanelData {
        data_blocked(report, None)
    }

    fn data_blocked(report: Option<Report>, blocked: Option<String>) -> ReportPanelData {
        ReportPanelData {
            week: week(),
            week_label: "Week 47 · Nov 16–22".into(),
            slots: vec![ReportSlot {
                kind: ReportKind::Summary,
                facts_md: "- Done: one thing".into(),
                preview: String::new(),
                versions: report.into_iter().collect(),
                streaming: None,
                blocked,
            }],
        }
    }

    fn view_editing(id: ReportId) -> ReportPanel {
        let mut v = ReportPanel::new();
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
    fn generate_asks_for_this_week_unless_blocked() {
        let mut v = ReportPanel::new();
        v.load(data(None));
        assert_eq!(
            v.update(Message::Generate),
            Action::Generate(ReportKind::Summary, week())
        );

        v.load(data_blocked(None, Some("Unlock to generate".into())));
        assert_eq!(v.update(Message::Generate), Action::None);
    }

    #[test]
    fn copy_prefers_the_edit() {
        let mut v = ReportPanel::new();
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
