//! Keyboard Shortcuts overlay (`?`).
//!
//! A restrained reference sheet matching Fortnight's `overlay_panel` style,
//! grouping navigation, task, and notes editor shortcuts alongside the
//! quick-add syntax hint.

use iced::widget::{Column, Space, button, column, container, row, rule, scrollable, text};
use iced::{Border, Center, Element, Fill, Font, Padding, Theme};

use crate::theme::{self, Icon};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    Close,
}

/// Quick-add syntax hint shown at the bottom of the sheet.
pub const QUICK_ADD_HINT: &str = "fri · due:fri · #project · ~2h · ! · every:weekly";

const NAVIGATION_SHORTCUTS: &[(&str, &str)] = &[
    ("1 / 2 / 3", "Today, Week, Month"),
    ("← / →", "Previous / Next period"),
    ("T", "Jump to today"),
    ("⌘F or /", "Search & filter"),
    ("⌘,", "Settings"),
    ("?", "Keyboard shortcuts"),
    ("Esc", "Close dialog / panel"),
];

const TASK_SHORTCUTS: &[(&str, &str)] = &[
    ("N or ⌘N", "Quick add / New task"),
    ("↑ / ↓", "Select task in Today"),
    ("Space", "Toggle done"),
    ("Enter", "Edit selected task"),
    ("⌘Z", "Undo"),
    ("⇧⌘Z", "Redo"),
];

const NOTES_SHORTCUTS: &[(&str, &str)] = &[
    ("⌘B / ⌘I / ⌘U", "Bold, italic, underline"),
    ("⌘K", "Insert / edit link"),
    ("Tab / ⇧Tab", "Indent / outdent list"),
    ("⌘Z / ⇧⌘Z", "Undo / redo text"),
];

/// Returns the styled `overlay_panel` container (`max_width(540)`), ready to
/// be centered inside a backdrop in `main.rs`.
pub fn view<'a>() -> Element<'a, Message> {
    let header = container(
        row![
            text("Keyboard shortcuts").size(theme::TEXT_HEADING),
            Space::new().width(Fill),
            button(theme::icon(Icon::Close))
                .padding(theme::ICON_PADDING)
                .style(theme::icon_button)
                .on_press(Message::Close),
        ]
        .align_y(Center),
    )
    .padding(Padding {
        top: theme::SPACING * 1.25,
        bottom: theme::SPACING,
        left: theme::SPACING * 1.75,
        right: theme::SPACING * 1.25,
    })
    .width(Fill);

    let groups = column![
        shortcut_group("Navigation & Views", NAVIGATION_SHORTCUTS),
        shortcut_group("Tasks", TASK_SHORTCUTS),
        shortcut_group("Notes Editor", NOTES_SHORTCUTS),
    ]
    .spacing(theme::SPACING * 1.5)
    .padding(Padding {
        top: theme::SPACING * 1.25,
        bottom: theme::SPACING * 1.25,
        left: theme::SPACING * 1.75,
        right: theme::SPACING * 1.75,
    })
    .width(Fill);

    let body = scrollable(groups)
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new()
                .width(6)
                .scroller_width(6)
                .margin(4),
        ))
        .height(iced::Length::Shrink);

    let quick_add = container(
        row![
            text("Quick add")
                .size(theme::TEXT_SMALL)
                .style(theme::muted),
            Space::new().width(Fill),
            text(QUICK_ADD_HINT)
                .size(12)
                .font(Font::MONOSPACE)
                .style(theme::muted),
        ]
        .spacing(theme::SPACING)
        .align_y(Center),
    )
    .padding([theme::SPACING * 0.9, theme::SPACING * 1.75])
    .width(Fill);

    let content = column![
        header,
        rule::horizontal(1).style(divider),
        container(body).max_height(520).width(Fill),
        rule::horizontal(1).style(divider),
        quick_add,
    ];

    container(content)
        .max_width(540)
        .width(Fill)
        .style(theme::overlay_panel)
        .into()
}

fn shortcut_group<'a>(heading: &'a str, items: &'a [(&'a str, &'a str)]) -> Element<'a, Message> {
    let mut col = Column::new()
        .spacing(theme::SPACING * 0.5)
        .push(theme::section_heading(heading));
    for (keys, label) in items {
        col = col.push(
            row![
                text(*label).size(theme::TEXT_SMALL).width(Fill),
                keycap(keys),
            ]
            .spacing(theme::SPACING)
            .align_y(Center),
        );
    }
    col.into()
}

fn keycap(keys: &str) -> Element<'_, Message> {
    container(text(keys).size(12).font(Font::MONOSPACE))
        .padding([2, 8])
        .style(|theme: &Theme| container::Style {
            background: Some(theme::tint(theme, 0.08).into()),
            text_color: Some(theme.palette().text),
            border: Border {
                radius: 5.0.into(),
                width: 1.0,
                color: theme::tint(theme, 0.18),
            },
            ..container::Style::default()
        })
        .into()
}

fn divider(theme: &Theme) -> rule::Style {
    rule::Style {
        color: theme::muted_color(theme).scale_alpha(0.25),
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_sheet_builds_view_and_lists_all_groups() {
        assert!(!NAVIGATION_SHORTCUTS.is_empty());
        assert!(!TASK_SHORTCUTS.is_empty());
        assert!(!NOTES_SHORTCUTS.is_empty());
        assert!(QUICK_ADD_HINT.contains("every:weekly"));
        let _ = view();
    }
}
