//! The report panel: Advice or Summary for one week, embedded by the Week
//! tab below its day columns. Owner: agent L (round 2).
//!
//! - A header with a Summary | Advice switch when the week has both, the
//!   version control ("Version 2 of 3", ‹ ›) and the "What will be sent"
//!   toggle.
//! - The report text (Markdown) beside "The facts" (Markdown), in one of
//!   these states: streaming ("Generating…", then the text so far with a
//!   quiet "Writing…"), none yet, blocked, failed (error + Retry), ready
//!   (Edit, Copy, Export as Markdown or JSON, Regenerate, metadata) and
//!   editing (`text_editor` + Save/Cancel).
//! - "What will be sent": `ReportSlot::preview` in a monospace scrollable
//!   block and, for an existing version, the facts it was built from
//!   (`Report::digest_json`).
//!
//! `load` runs after every message and on every streamed piece, so it
//! returns early on equal data and only reparses Markdown whose source
//! changed; streamed text that grows is pushed incrementally.
//!
//! Skipped (prototype scope): a date pick list for versions (‹ › only),
//! diffing versions, keyboard shortcuts inside the panel, and copying the
//! "what will be sent" text.

use fortnight_core::domain::{Report, ReportId, ReportKind, ReportStatus, WeekRef};
use fortnight_core::export::ExportFormat;
use iced::widget::{
    Column, Row, button, column, container, markdown, row, scrollable, space, text, text_editor,
};
use iced::{Center, Element, Fill, FillPortion, Font, Padding, border, color};

use crate::theme;

#[derive(Debug, Clone)]
pub enum Message {
    /// Show the slot of this kind.
    ShowKind(ReportKind),
    /// Show the next older version of the slot.
    OlderVersion,
    /// Show the next newer version of the slot.
    NewerVersion,
    Generate,
    StartEdit,
    EditAction(text_editor::Action),
    SaveEdit,
    CancelEdit,
    Copy,
    /// Export the shown version in this format.
    Export(ExportFormat),
    /// Show or hide "what will be sent".
    ToggleSent,
    /// Pick what the "what will be sent" block shows.
    ShowSent(Sent),
    /// A Markdown link was clicked; ignored on purpose.
    LinkClicked(markdown::Uri),
}

/// What the "what will be sent" block shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sent {
    /// `ReportSlot::preview`: what generating now would send.
    #[default]
    Now,
    /// The shown version's `Report::digest_json`.
    ShownVersion,
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
    /// The version shown; `None` means the newest.
    shown: Option<ReportId>,
    /// The shown slot's facts, parsed, and the source they came from.
    facts: markdown::Content,
    facts_src: String,
    /// The report text shown (the streamed text while generating,
    /// otherwise the shown version's text), parsed, and its source.
    body: markdown::Content,
    body_src: String,
    /// How many times `body` was (re)parsed; for tests.
    body_parses: u32,
    /// The open editor and the version it edits.
    editor: Option<(ReportId, text_editor::Content)>,
    /// "What will be sent" is open.
    sent_open: bool,
    sent: Sent,
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
        let same_week = old.as_ref().map(|d| d.week) == Some(data.week);
        if same_week {
            let old_slot = old.as_ref().and_then(|d| slot_of(d, self.kind));
            let new_slot = slot_of(&data, self.kind);
            let key = |s: Option<&ReportSlot>| s.map(|s| (s.kind, newest_id(s)));
            if key(old_slot) != key(new_slot) {
                // Another slot, or a new version arrived: jump to the newest.
                self.shown = None;
                self.editor = None;
            }
        } else {
            self.kind = None;
            self.shown = None;
            self.editor = None;
            self.sent = Sent::Now;
        }
        if let Some(k) = self.kind
            && !data.slots.iter().any(|s| s.kind == k)
        {
            self.kind = None;
            self.shown = None;
            self.editor = None;
        }
        let versions = slot_of(&data, self.kind).map_or(&[][..], |s| &s.versions);
        let exists = |id: ReportId| versions.iter().any(|r| r.id == id);
        if self.shown.is_some_and(|id| !exists(id)) {
            self.shown = None;
        }
        if self.editor.as_ref().is_some_and(|(id, _)| !exists(*id)) {
            self.editor = None;
        }
        self.data = Some(data);
        self.refresh();
    }

    /// Reparses the facts and the body if their sources changed. Streamed
    /// text that only grew is pushed incrementally.
    fn refresh(&mut self) {
        let (facts_src, body_src) = match self.slot() {
            Some(slot) => {
                let body = match &slot.streaming {
                    Some(so_far) => so_far.clone(),
                    None => self
                        .shown_report()
                        .and_then(Report::text)
                        .unwrap_or_default()
                        .to_owned(),
                };
                (slot.facts_md.clone(), body)
            }
            None => (String::new(), String::new()),
        };
        if facts_src != self.facts_src {
            self.facts = markdown::Content::parse(&facts_src);
            self.facts_src = facts_src;
        }
        if body_src != self.body_src {
            match body_src.strip_prefix(self.body_src.as_str()) {
                Some(more) if !self.body_src.is_empty() => self.body.push_str(more),
                _ => self.body = markdown::Content::parse(&body_src),
            }
            self.body_src = body_src;
            self.body_parses += 1;
        }
    }

    fn slot(&self) -> Option<&ReportSlot> {
        slot_of(self.data.as_ref()?, self.kind)
    }

    /// Index of the shown version in the slot's versions (0 = newest).
    fn shown_index(&self) -> usize {
        let Some(slot) = self.slot() else { return 0 };
        self.shown
            .and_then(|id| slot.versions.iter().position(|r| r.id == id))
            .unwrap_or(0)
    }

    /// The version shown, if the slot has any.
    fn shown_report(&self) -> Option<&Report> {
        self.slot()?.versions.get(self.shown_index())
    }

    /// The text in the open editor, if editing.
    pub fn editor_text(&self) -> Option<String> {
        self.editor.as_ref().map(|(_, c)| c.text())
    }

    pub fn update(&mut self, message: Message) -> Action {
        match message {
            Message::ShowKind(kind) => {
                if self.slot().map(|s| s.kind) != Some(kind) {
                    self.kind = Some(kind);
                    self.shown = None;
                    self.editor = None;
                    self.refresh();
                }
                Action::None
            }
            Message::OlderVersion | Message::NewerVersion => {
                let Some(slot) = self.slot() else {
                    return Action::None;
                };
                let i = self.shown_index();
                let j = match message {
                    Message::OlderVersion => (i + 1).min(slot.versions.len().saturating_sub(1)),
                    _ => i.saturating_sub(1),
                };
                if j != i {
                    self.shown = slot.versions.get(j).map(|r| r.id);
                    self.editor = None;
                    self.refresh();
                }
                Action::None
            }
            Message::Generate => {
                let Some(data) = &self.data else {
                    return Action::None;
                };
                match slot_of(data, self.kind) {
                    Some(slot) if slot.blocked.is_none() && slot.streaming.is_none() => {
                        let action = Action::Generate(slot.kind, data.week);
                        self.editor = None;
                        action
                    }
                    _ => Action::None,
                }
            }
            Message::StartEdit => {
                if let Some(r) = self.shown_report()
                    && let Some(text) = r.text()
                {
                    self.editor = Some((r.id, text_editor::Content::with_text(text)));
                }
                Action::None
            }
            Message::EditAction(action) => {
                if let Some((_, editor)) = &mut self.editor {
                    editor.perform(action);
                }
                Action::None
            }
            Message::SaveEdit => match self.editor.take() {
                Some((id, editor)) => Action::SaveEdit(id, editor.text()),
                None => Action::None,
            },
            Message::CancelEdit => {
                self.editor = None;
                Action::None
            }
            Message::Copy => match self.shown_report().and_then(Report::text) {
                Some(text) => Action::Copy(text.to_owned()),
                None => Action::None,
            },
            Message::Export(format) => match self.shown_report() {
                Some(r) if r.status == ReportStatus::Ready => Action::Export(r.id, format),
                _ => Action::None,
            },
            Message::ToggleSent => {
                self.sent_open = !self.sent_open;
                Action::None
            }
            Message::ShowSent(sent) => {
                self.sent = sent;
                Action::None
            }
            Message::LinkClicked(_) => Action::None,
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let Some(data) = &self.data else {
            return text("Loading…").style(theme::muted).into();
        };
        let Some(slot) = self.slot() else {
            return Column::new().into();
        };

        let mut col = column![self.header(data, slot)].spacing(theme::SPACING * 1.5);
        if self.sent_open {
            col = col.push(self.sent_view(slot));
        }

        let report = column![self.body_view(slot)]
            .spacing(theme::SPACING)
            .width(FillPortion(3))
            .max_width(720);
        let facts = column![
            theme::section_heading("The facts"),
            container(md(&self.facts, theme::TEXT_SMALL + 1.0))
                .padding(theme::SPACING * 1.25)
                .width(Fill)
                .style(inset),
        ]
        .spacing(theme::SPACING / 2.0)
        .width(FillPortion(2))
        .max_width(460);

        col.push(row![report, facts].spacing(theme::SPACING * 2.5))
            .into()
    }

    /// The kind switch (or name), the version control and the "what will be
    /// sent" toggle.
    fn header<'a>(
        &'a self,
        data: &'a ReportPanelData,
        slot: &'a ReportSlot,
    ) -> Element<'a, Message> {
        let kind: Element<'a, Message> = if data.slots.len() > 1 {
            let mut tabs = Row::new().spacing(2);
            for s in &data.slots {
                tabs = tabs.push(
                    button(text(kind_label(s.kind)).size(theme::TEXT_SMALL + 1.0))
                        .padding(theme::tab_padding())
                        .style(theme::tab_button(s.kind == slot.kind))
                        .on_press(Message::ShowKind(s.kind)),
                );
            }
            container(tabs).padding(3).style(theme::tab_bar).into()
        } else {
            text(kind_label(slot.kind)).size(theme::TEXT_HEADING).into()
        };

        let mut header = row![kind, space::horizontal()]
            .spacing(theme::SPACING)
            .align_y(Center);

        let count = slot.versions.len();
        if count > 1 && slot.streaming.is_none() {
            let i = self.shown_index();
            let failed = slot.versions[i].status == ReportStatus::Failed;
            let mut label = format!("Version {} of {count}", count - i);
            if failed {
                label.push_str(" · failed");
            }
            let chevron = |icon, enabled: bool, msg| {
                button(theme::icon(icon))
                    .padding(theme::ICON_PADDING)
                    .style(theme::icon_button)
                    .on_press_maybe(enabled.then_some(msg))
            };
            header = header.push(
                row![
                    chevron(
                        theme::Icon::ChevronLeft,
                        i + 1 < count,
                        Message::OlderVersion
                    ),
                    text(label).size(theme::TEXT_SMALL).style(if failed {
                        theme::danger
                    } else {
                        theme::muted
                    }),
                    chevron(theme::Icon::ChevronRight, i > 0, Message::NewerVersion),
                ]
                .spacing(2)
                .align_y(Center),
            );
        }

        let toggle = if self.sent_open {
            "Hide what will be sent"
        } else {
            "What will be sent"
        };
        header
            .push(
                button(text(toggle).size(theme::TEXT_SMALL))
                    .padding([4, 8])
                    .style(theme::text_button)
                    .on_press(Message::ToggleSent),
            )
            .into()
    }

    /// The report text and its actions, by state.
    fn body_view<'a>(&'a self, slot: &'a ReportSlot) -> Element<'a, Message> {
        let blocked_hint =
            |reason: &'a str| text(reason).size(theme::TEXT_SMALL).style(theme::muted);

        // Streaming.
        if let Some(so_far) = &slot.streaming {
            if so_far.is_empty() {
                return text("Generating…").style(theme::muted).into();
            }
            return column![
                md(&self.body, theme::TEXT_BODY),
                text("Writing…").size(theme::TEXT_SMALL).style(theme::muted),
            ]
            .spacing(theme::SPACING)
            .into();
        }

        // None yet (or blocked with none yet).
        let Some(report) = self.shown_report() else {
            let mut generate = button(text("Generate").size(theme::TEXT_BODY))
                .padding([8, 16])
                .style(theme::primary_button);
            let mut col =
                column![text(empty_note(slot.kind)).style(theme::muted)].spacing(theme::SPACING);
            match &slot.blocked {
                Some(reason) => col = col.push(blocked_hint(reason)),
                None => generate = generate.on_press(Message::Generate),
            }
            return col.push(generate).into();
        };

        // Failed.
        if report.status == ReportStatus::Failed {
            let error = report.error.as_deref().unwrap_or("Generation failed.");
            let retry = button(text("Retry").size(theme::TEXT_BODY))
                .padding([8, 16])
                .style(theme::primary_button)
                .on_press_maybe(slot.blocked.is_none().then_some(Message::Generate));
            let mut col = column![
                text("This version failed.").style(theme::muted),
                text(error).style(theme::danger),
                retry,
            ]
            .spacing(theme::SPACING);
            if let Some(reason) = &slot.blocked {
                col = col.push(blocked_hint(reason));
            }
            return col
                .push(
                    text(metadata(report))
                        .size(theme::TEXT_SMALL)
                        .style(theme::muted),
                )
                .into();
        }

        // Editing.
        if let Some((_, editor)) = &self.editor {
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

        // Ready.
        let action = |label, msg: Option<Message>| {
            button(text(label).size(theme::TEXT_SMALL + 1.0))
                .padding([6, 12])
                .style(theme::secondary_button)
                .on_press_maybe(msg)
        };
        let buttons = row![
            action("Edit", Some(Message::StartEdit)),
            action("Copy", Some(Message::Copy)),
            action("Export .md", Some(Message::Export(ExportFormat::Markdown))),
            action("Export .json", Some(Message::Export(ExportFormat::Json))),
            space::horizontal(),
            action(
                "Regenerate",
                slot.blocked.is_none().then_some(Message::Generate)
            ),
        ]
        .spacing(theme::SPACING / 2.0)
        .align_y(Center);

        let mut col = column![
            md(&self.body, theme::TEXT_BODY),
            text(metadata(report))
                .size(theme::TEXT_SMALL)
                .style(theme::muted),
            buttons,
        ]
        .spacing(theme::SPACING);
        if let Some(reason) = &slot.blocked {
            col = col.push(blocked_hint(reason));
        }
        col.into()
    }

    /// "What will be sent": the request preview or the shown version's
    /// facts, monospace and scrollable.
    fn sent_view<'a>(&'a self, slot: &'a ReportSlot) -> Element<'a, Message> {
        let report = self.shown_report();
        let sent = if report.is_some() {
            self.sent
        } else {
            Sent::Now
        };
        let (note, body) = match (sent, report) {
            (Sent::ShownVersion, Some(r)) => {
                ("The facts sent for this version.", r.digest_json.as_str())
            }
            _ => (
                "What Generate would send now. Private tasks are left out.",
                slot.preview.as_str(),
            ),
        };

        let mut top = row![
            text(note).size(theme::TEXT_SMALL).style(theme::muted),
            space::horizontal()
        ]
        .spacing(theme::SPACING)
        .align_y(Center);
        if report.is_some() {
            let tab = |label, which: Sent| {
                button(text(label).size(theme::TEXT_SMALL))
                    .padding([3, 10])
                    .style(theme::tab_button(sent == which))
                    .on_press(Message::ShowSent(which))
            };
            top = top.push(
                container(
                    row![
                        tab("If generated now", Sent::Now),
                        tab("This version", Sent::ShownVersion),
                    ]
                    .spacing(2),
                )
                .padding(3)
                .style(theme::tab_bar),
            );
        }

        let block = scrollable(
            container(
                text(body)
                    .font(Font::MONOSPACE)
                    .size(theme::TEXT_SMALL - 1.0),
            )
            .padding(theme::SPACING)
            .width(Fill),
        )
        .height(320);

        column![top, container(block).width(Fill).style(inset)]
            .spacing(theme::SPACING / 2.0)
            .into()
    }
}

/// The slot of `kind`, or the first slot.
fn slot_of(data: &ReportPanelData, kind: Option<ReportKind>) -> Option<&ReportSlot> {
    kind.and_then(|k| data.slots.iter().find(|s| s.kind == k))
        .or(data.slots.first())
}

fn newest_id(slot: &ReportSlot) -> Option<ReportId> {
    slot.versions.first().map(|r| r.id)
}

fn kind_label(kind: ReportKind) -> &'static str {
    match kind {
        ReportKind::Summary => "Summary",
        ReportKind::Plan => "Advice",
    }
}

fn empty_note(kind: ReportKind) -> &'static str {
    match kind {
        ReportKind::Summary => {
            "No summary for this week yet. Generate turns the facts into an update for your manager."
        }
        ReportKind::Plan => {
            "No advice for this week yet. Generate asks the model what is at risk, what order to work in and what to cut."
        }
    }
}

/// The facts and "what will be sent" blocks: a fill a little stronger than
/// `theme::card`, so they read on the Week tab's card.
fn inset(theme: &iced::Theme) -> container::Style {
    let p = theme.palette();
    let t = 0.07;
    container::Style {
        background: Some(
            iced::Color::from_rgb(
                p.background.r + (p.text.r - p.background.r) * t,
                p.background.g + (p.text.g - p.background.g) * t,
                p.background.b + (p.text.b - p.background.b) * t,
            )
            .into(),
        ),
        border: border::rounded(theme::RADIUS),
        ..container::Style::default()
    }
}

/// Renders parsed Markdown with the app's font and accent; links are
/// reported as `LinkClicked` and ignored.
fn md(content: &markdown::Content, size: f32) -> Element<'_, Message> {
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
    // iced's defaults (h1 twice the body) are too loud inside a panel.
    let settings = markdown::Settings {
        h1_size: (size * 1.4).into(),
        h2_size: (size * 1.2).into(),
        h3_size: (size * 1.1).into(),
        h4_size: size.into(),
        code_size: (size * 0.9).into(),
        spacing: (size * 0.75).into(),
        ..markdown::Settings::with_text_size(size, style)
    };
    markdown::view(content.items(), settings).map(Message::LinkClicked)
}

/// "gpt-4o-mini · Nov 20, 16:02 · edited · prompt v3".
fn metadata(report: &Report) -> String {
    let when = report
        .created_at
        .to_zoned_datetime(jiff::tz::TimeZone::system())
        .strftime("%b %-d, %H:%M")
        .to_string();
    let mut s = match &report.model {
        Some(model) => format!("{model} · {when}"),
        None => when,
    };
    if report.edited_md.is_some() {
        s.push_str(" · edited");
    }
    s.push_str(&format!(" · prompt v{}", report.prompt_version));
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
            digest_json: format!("{{\"version\": {id}}}"),
            content_md: Some("## Highlights\n- Shipped it".into()),
            edited_md: None,
            model: Some("test-model".into()),
            endpoint_host: None,
            prompt_version: 1,
            error: None,
            created_at: Timestamp::UNIX_EPOCH,
        }
    }

    fn report_text(id: ReportId, text: &str) -> Report {
        Report {
            content_md: Some(text.into()),
            ..report(id)
        }
    }

    fn slot(kind: ReportKind, versions: Vec<Report>) -> ReportSlot {
        ReportSlot {
            kind,
            facts_md: format!("- Facts for {kind:?}"),
            preview: format!("System prompt\n---\npreview for {kind:?}"),
            versions,
            streaming: None,
            blocked: None,
        }
    }

    fn data_slots(slots: Vec<ReportSlot>) -> ReportPanelData {
        ReportPanelData {
            week: week(),
            week_label: "Week 47 · Nov 16 to 22".into(),
            slots,
        }
    }

    fn data(report: Option<Report>) -> ReportPanelData {
        data_blocked(report, None)
    }

    fn data_blocked(report: Option<Report>, blocked: Option<String>) -> ReportPanelData {
        let mut s = slot(ReportKind::Summary, report.into_iter().collect());
        s.facts_md = "- Done: one thing".into();
        s.blocked = blocked;
        data_slots(vec![s])
    }

    /// A summary slot with versions 3, 2, 1 (newest first).
    fn three_versions() -> ReportPanelData {
        data_slots(vec![slot(
            ReportKind::Summary,
            vec![
                report_text(3, "third"),
                report_text(2, "second"),
                report_text(1, "first"),
            ],
        )])
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
    fn load_with_unrelated_change_keeps_editor_text() {
        let mut v = view_editing(1);
        let typed = v.editor_text().unwrap();
        let mut d = data(Some(report(1)));
        d.slots[0].facts_md = "- Done: two things".into();
        v.load(d);
        assert_eq!(v.editor_text().as_deref(), Some(typed.as_str()));
    }

    #[test]
    fn load_with_new_version_closes_editor() {
        let mut v = view_editing(1);
        v.load(data_slots(vec![slot(
            ReportKind::Summary,
            vec![report(2), report(1)],
        )]));
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
    fn generate_is_disabled_while_streaming() {
        let mut v = ReportPanel::new();
        let mut d = data(Some(report(1)));
        d.slots[0].streaming = Some(String::new());
        v.load(d);
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
    fn save_targets_the_shown_older_version() {
        let mut v = ReportPanel::new();
        v.load(three_versions());
        v.update(Message::OlderVersion);
        v.update(Message::StartEdit);
        assert_eq!(v.editor_text().as_deref(), Some("second"));
        assert_eq!(
            v.update(Message::SaveEdit),
            Action::SaveEdit(2, "second".into())
        );
    }

    #[test]
    fn cancel_discards_the_edit() {
        let mut v = view_editing(7);
        assert_eq!(v.update(Message::CancelEdit), Action::None);
        assert_eq!(v.editor_text(), None);
        assert_eq!(v.update(Message::SaveEdit), Action::None);
    }

    #[test]
    fn switching_slots_shows_the_other_kind() {
        let mut v = ReportPanel::new();
        let mut plan = report_text(9, "advice text");
        plan.kind = ReportKind::Plan;
        v.load(data_slots(vec![
            slot(ReportKind::Summary, vec![report_text(1, "summary text")]),
            slot(ReportKind::Plan, vec![plan]),
        ]));
        assert_eq!(v.update(Message::Copy), Action::Copy("summary text".into()));
        assert_eq!(v.facts_src, "- Facts for Summary");

        v.update(Message::StartEdit);
        v.update(Message::ShowKind(ReportKind::Plan));
        assert_eq!(v.editor_text(), None, "switching closes the editor");
        assert_eq!(v.update(Message::Copy), Action::Copy("advice text".into()));
        assert_eq!(v.body_src, "advice text");
        assert_eq!(v.facts_src, "- Facts for Plan");
        assert_eq!(
            v.update(Message::Generate),
            Action::Generate(ReportKind::Plan, week())
        );
    }

    #[test]
    fn version_navigation_and_kept_across_load() {
        let mut v = ReportPanel::new();
        v.load(three_versions());
        assert_eq!(v.update(Message::Copy), Action::Copy("third".into()));
        v.update(Message::NewerVersion); // already newest: stays
        assert_eq!(v.shown_index(), 0);
        v.update(Message::OlderVersion);
        v.update(Message::OlderVersion);
        v.update(Message::OlderVersion); // already oldest: stays
        assert_eq!(v.update(Message::Copy), Action::Copy("first".into()));
        assert_eq!(v.body_src, "first");
        v.update(Message::NewerVersion);
        assert_eq!(v.update(Message::Copy), Action::Copy("second".into()));

        // An unrelated change keeps the shown version.
        let mut d = three_versions();
        d.slots[0].facts_md = "- other facts".into();
        v.load(d);
        assert_eq!(v.update(Message::Copy), Action::Copy("second".into()));
    }

    #[test]
    fn new_version_jumps_to_newest() {
        let mut v = ReportPanel::new();
        v.load(three_versions());
        v.update(Message::OlderVersion);
        assert_eq!(v.update(Message::Copy), Action::Copy("second".into()));

        let mut d = three_versions();
        d.slots[0].versions.insert(0, report_text(4, "fourth"));
        v.load(d);
        assert_eq!(v.shown_index(), 0);
        assert_eq!(v.update(Message::Copy), Action::Copy("fourth".into()));
    }

    #[test]
    fn failed_versions_are_listed_but_not_exported() {
        let mut v = ReportPanel::new();
        let failed = Report {
            status: ReportStatus::Failed,
            content_md: None,
            error: Some("timed out".into()),
            ..report(2)
        };
        v.load(data_slots(vec![slot(
            ReportKind::Summary,
            vec![failed, report(1)],
        )]));
        assert_eq!(
            v.update(Message::Export(ExportFormat::Markdown)),
            Action::None
        );
        assert_eq!(v.update(Message::Copy), Action::None);
        v.update(Message::OlderVersion);
        assert_eq!(
            v.update(Message::Export(ExportFormat::Markdown)),
            Action::Export(1, ExportFormat::Markdown)
        );
    }

    #[test]
    fn export_in_both_formats_acts_on_the_shown_version() {
        let mut v = ReportPanel::new();
        v.load(three_versions());
        assert_eq!(
            v.update(Message::Export(ExportFormat::Markdown)),
            Action::Export(3, ExportFormat::Markdown)
        );
        v.update(Message::OlderVersion);
        assert_eq!(
            v.update(Message::Export(ExportFormat::Json)),
            Action::Export(2, ExportFormat::Json)
        );
    }

    #[test]
    fn nothing_to_export_or_copy_without_a_version() {
        let mut v = ReportPanel::new();
        v.load(data(None));
        assert_eq!(v.update(Message::Export(ExportFormat::Json)), Action::None);
        assert_eq!(v.update(Message::Copy), Action::None);
        v.update(Message::StartEdit);
        assert_eq!(v.editor_text(), None);
    }

    #[test]
    fn streaming_text_is_reparsed_only_when_it_changes() {
        let streaming = |so_far: &str, facts: &str| {
            let mut d = data(None);
            d.slots[0].streaming = Some(so_far.into());
            d.slots[0].facts_md = facts.into();
            d
        };
        let mut v = ReportPanel::new();
        v.load(streaming("", "f"));
        let start = v.body_parses;

        v.load(streaming("## High", "f"));
        assert_eq!(v.body_parses, start + 1);
        assert_eq!(v.body_src, "## High");

        v.load(streaming("## High", "f"));
        v.load(streaming("## High", "other facts"));
        assert_eq!(v.body_parses, start + 1, "facts change alone");

        v.load(streaming("## Highlights\n- one", "other facts"));
        assert_eq!(v.body_parses, start + 2);
        assert!(!v.body.items().is_empty());

        // The finished version has the same text: no reparse.
        let mut done = data(Some(report_text(1, "## Highlights\n- one")));
        done.slots[0].facts_md = "other facts".into();
        v.load(done);
        assert_eq!(v.body_parses, start + 2);
    }

    #[test]
    fn incremental_push_matches_a_full_parse() {
        let mut v = ReportPanel::new();
        let text = "## Highlights\n- one\n- two\n\n## Next week\nMore.";
        for end in [3, 10, 20, text.len()] {
            let mut d = data(None);
            d.slots[0].streaming = Some(text[..end].into());
            v.load(d);
        }
        assert_eq!(
            v.body.items().len(),
            markdown::Content::parse(text).items().len()
        );
    }

    #[test]
    fn preview_toggle_and_version_facts() {
        let mut v = ReportPanel::new();
        v.load(three_versions());
        assert!(!v.sent_open);
        v.update(Message::ToggleSent);
        assert!(v.sent_open);
        assert_eq!(v.sent, Sent::Now);
        v.update(Message::ShowSent(Sent::ShownVersion));
        assert_eq!(v.sent, Sent::ShownVersion);
        // The view builds in both modes.
        let _ = v.view();
        v.update(Message::ShowSent(Sent::Now));
        let _ = v.view();
        // Stays open across loads.
        let mut d = three_versions();
        d.slots[0].preview = "changed".into();
        v.load(d);
        assert!(v.sent_open);
        v.update(Message::ToggleSent);
        assert!(!v.sent_open);
    }

    #[test]
    fn every_state_builds_a_view() {
        let mut v = ReportPanel::new();
        let _ = v.view(); // before load
        v.load(data(None));
        let _ = v.view();
        v.load(data_blocked(None, Some("Unlock".into())));
        let _ = v.view();
        let mut d = data(None);
        d.slots[0].streaming = Some(String::new());
        v.load(d.clone());
        let _ = v.view();
        d.slots[0].streaming = Some("## Hi".into());
        v.load(d);
        let _ = v.view();
        v.load(three_versions());
        let _ = v.view();
        v.update(Message::StartEdit);
        let _ = v.view();
        let failed = Report {
            status: ReportStatus::Failed,
            content_md: None,
            error: Some("timed out".into()),
            ..report(5)
        };
        v.load(data(Some(failed)));
        let _ = v.view();
    }
}
