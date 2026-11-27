//! Theme and shared styles. Owner: orchestrator (round 2; was agent E).
//! Views may add private style helpers in their own files.
//!
//! Light, dark, or following the system (Settings' Appearance). One accent colour, picked in
//! Settings (orange `#ea580c` by default; [`accent_color`]), used sparingly
//! (primary buttons, selected tabs, focus rings, checked boxes). Generous spacing,
//! no borders where whitespace will do. Other views use the constants and
//! style functions here so the app looks like one thing.
//!
//! # Picking light or dark
//!
//! `main.rs` keeps the system appearance (`Option<bool>`, `Some(true)` =
//! dark), asks the system once at launch with `iced::system::theme()` (a
//! `Task<iced::theme::Mode>`), follows changes with the subscription
//! `iced::system::theme_changes()`, and returns
//! `Some(theme_for(is_dark(settings.appearance, system)))` from its `theme` fn.
//!
//! # Constants
//!
//! - [`accent_color`]: the accent colour in use (Settings picks it, [`set_accent`]).
//! - [`SPACING`]: the base spacing unit; use multiples of it.
//! - [`RADIUS`]: corner radius for cards, inputs and buttons.
//! - [`TEXT_SMALL`], [`TEXT_BODY`], [`TEXT_HEADING`], [`TEXT_TITLE`]: sizes.
//! - [`CONTENT_WIDTH`]: max width of a view's main column.
//!
//! # Colours (for `.color(...)` or custom styles)
//!
//! - [`muted_color`]: secondary text (hints, labels, done tasks).
//! - [`danger_color`]: errors and overdue dates.
//!
//! # Text styles (`text(..).style(theme::muted)`)
//!
//! - [`muted`]: secondary text.
//! - [`danger`]: errors, overdue.
//! - [`accent`]: the accent colour; use rarely.
//!
//! # Text helpers (return a ready-made `Text`)
//!
//! - [`title`]: a view's title ("Tasks", "Week 46").
//! - [`section_heading`]: a small muted heading above a group.
//!
//! # Containers (`container(..).style(theme::card)`)
//!
//! - [`card`]: a softly filled rounded panel, no border.
//! - [`chip`]: a small pill, e.g. a project tag. Use with [`chip_padding`].
//!
//! # Buttons (`button(..).style(theme::primary_button)`)
//!
//! - [`primary_button`]: the one main action on screen; accent fill.
//! - [`secondary_button`]: other actions; soft neutral fill.
//! - [`text_button`]: link-like; muted text, no fill.
//! - [`danger_button`]: destructive action as red text, no fill.
//!
//! # Inputs
//!
//! - [`input`]: `text_input(..).style(theme::input)`; soft fill, accent focus.
//! - [`editor`]: the same look for `text_editor`.
//!
//! # Round 2: navigation and calendar
//!
//! - [`top_bar`]: the slim bar across the top (container).
//! - [`tab_bar`] + [`tab_button`]: the Today | Week | Month segmented control;
//!   [`tab_pill`]: its selected fill, where it slides between tabs.
//! - [`icon`] + [`icon_button`]: SVG icons ([`Icon`]) in square, fill-on-hover
//!   buttons (lock, gear, chevrons).
//! - [`day_column`]: a column of the Week tab; `is_today` adds the accent ring.
//! - [`calendar_cell`]: a Month grid cell, by [`Cell`] (today, weekend,
//!   outside the month).
//! - [`load_bar`] + [`load_bar_done`]: `progress_bar` styles for the Week
//!   load bar: capacity track, gray task share, accent done.
//! - [`overlay_panel`]: the panel of an overlay (Settings, a dialog), over a
//!   dimmed layer (`motion::dim`) at [`BACKDROP`], or [`DIALOG_BACKDROP`]
//!   for the task dialog.
//!
//! # Shared helpers
//!
//! - [`tint`] and [`mix`]: neutral fills mixed from the theme's colours.
//! - [`tip`]: a tooltip's bubble.
//! - [`FONT_BOLD`]: Inter, bold.
//! - [`title_with_marks`] and [`priority_marks`]: a task title with its
//!   priority marks.

use fortnight_core::domain::{Accent, Appearance, Priority, TodayWidth};
use iced::border::{self, Border};
use iced::theme::{Palette, palette};
use iced::widget::{
    button, container, progress_bar, rich_text, span, stack, svg, text, text_editor, text_input,
};
use iced::{Background, Color, Element, Padding, Theme, color};
use std::sync::atomic::{AtomicU32, Ordering};

/// The accent colour in use (`0xRRGGBB`): Settings' choice, which
/// `main.rs` applies with [`set_accent`]. Orange until then.
static ACCENT_RGB: AtomicU32 = AtomicU32::new(0xea580c);

/// Each accent choice's colour as `0xRRGGBB`.
fn accent_rgb(accent: Accent) -> u32 {
    match accent {
        Accent::Orange => 0xea580c,
        Accent::Emerald => 0x06885a,
        Accent::Teal => 0x308480,
        Accent::Blue => 0x0474e9,
        Accent::Purple => 0x7864de,
        Accent::Magenta => 0xb54eb2,
        Accent::Red => 0xdc3552,
    }
}

fn rgb_color(rgb: u32) -> Color {
    Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// The colour of an accent choice (for the Settings swatches).
pub fn choice_color(accent: Accent) -> Color {
    rgb_color(accent_rgb(accent))
}

/// Uses `accent` everywhere from the next frame on.
pub fn set_accent(accent: Accent) {
    ACCENT_RGB.store(accent_rgb(accent), Ordering::Relaxed);
}

/// The accent colour in use: selected tabs, primary buttons, checkboxes,
/// badges, focus rings. Orange unless Settings picked another.
pub fn accent_color() -> Color {
    rgb_color(ACCENT_RGB.load(Ordering::Relaxed))
}

/// The app's one typeface, Inter, bundled and loaded by `main.rs`. Use it
/// wherever a widget sets a font explicitly (e.g. `markdown::Style::font`):
/// `Font::default()` is the generic sans-serif family, whose bold falls
/// back to a monospace face on macOS.
pub const FONT: iced::Font = iced::Font::with_name("Inter");

/// [`FONT`] in bold.
pub const FONT_BOLD: iced::Font = iced::Font {
    weight: iced::font::Weight::Bold,
    ..FONT
};

/// Standard spacing unit, in logical pixels.
pub const SPACING: f32 = 12.0;

/// Corner radius for cards, inputs and buttons.
pub const RADIUS: f32 = 8.0;

/// Small text: hints, chips, due labels, section headings.
pub const TEXT_SMALL: f32 = 13.0;
/// Body text.
pub const TEXT_BODY: f32 = 15.0;
/// Card or sub-section headings.
pub const TEXT_HEADING: f32 = 18.0;
/// A view's title.
pub const TEXT_TITLE: f32 = 26.0;

/// Maximum width of a view's main column, so lines stay readable.
pub const CONTENT_WIDTH: f32 = 760.0;

/// The Today column's width (`TodayWidth`'s pixels), which `main.rs`
/// applies with [`set_today_width`]. [`CONTENT_WIDTH`] until then.
static TODAY_WIDTH: AtomicU32 = AtomicU32::new(760);

/// Use this Today width from now on (Settings' choice).
pub fn set_today_width(width: TodayWidth) {
    TODAY_WIDTH.store(
        match width {
            TodayWidth::Standard => 760,
            TodayWidth::Wide => 980,
            TodayWidth::ExtraWide => 1240,
        },
        Ordering::Relaxed,
    );
}

/// Maximum width of the Today tab's column: Settings' Today width.
pub fn today_width() -> f32 {
    TODAY_WIDTH.load(Ordering::Relaxed) as f32
}

const LIGHT: Palette = Palette {
    background: color!(0xfcfcfb),
    text: color!(0x1c1917),
    // Replaced by the accent in use (`theme_for`).
    primary: color!(0xea580c),
    success: color!(0x15803d),
    warning: color!(0xb45309),
    danger: color!(0xdc2626),
};

const DARK: Palette = Palette {
    background: color!(0x1a1918),
    text: color!(0xe7e5e4),
    // Replaced by the accent in use (`theme_for`).
    primary: color!(0xea580c),
    success: color!(0x4ade80),
    warning: color!(0xfbbf24),
    danger: color!(0xf87171),
};

/// Whether to use the dark theme: Settings' choice, or the system
/// appearance for "Match system" (light when the system doesn't say).
pub fn is_dark(choice: Appearance, system_dark: Option<bool>) -> bool {
    match choice {
        Appearance::Light => false,
        Appearance::Dark => true,
        Appearance::System => system_dark.unwrap_or(false),
    }
}

/// The app theme for light (`dark = false`) or dark mode, with the accent.
/// `main.rs` picks `dark` with [`is_dark`] (see the module doc).
pub fn theme_for(dark: bool) -> Theme {
    let (name, base) = if dark {
        ("Fortnight Dark", DARK)
    } else {
        ("Fortnight Light", LIGHT)
    };
    let palette = Palette {
        primary: accent_color(),
        ..base
    };
    Theme::custom_with_fn(name, palette, extended)
}

/// iced's generated palette, with white text on the accent and the
/// background shades replaced by plain mixes of background and text
/// (iced's own drift warm in light mode).
fn extended(palette: Palette) -> palette::Extended {
    let mut ext = palette::Extended::generate(palette);
    // White on the accent (checkmarks, primary buttons) in both modes.
    ext.primary.base.text = Color::WHITE;
    ext.primary.strong.text = Color::WHITE;
    let shade = |amount: f32| {
        palette::Pair::new(mix(palette.background, palette.text, amount), palette.text)
    };
    ext.background = palette::Background {
        base: palette::Pair::new(palette.background, palette.text),
        weakest: shade(0.03),
        weaker: shade(0.05),
        weak: shade(0.075),
        neutral: shade(0.10),
        strong: shade(0.15),
        stronger: shade(0.20),
        strongest: shade(0.25),
    };
    ext
}

/// `a` moved `amount` (0–1) towards `b`, opaque.
pub fn mix(a: Color, b: Color, amount: f32) -> Color {
    Color::from_rgb(
        a.r + (b.r - a.r) * amount,
        a.g + (b.g - a.g) * amount,
        a.b + (b.b - a.b) * amount,
    )
}

// ---------------------------------------------------------------- colours

/// Secondary text colour: hints, labels, done tasks.
pub fn muted_color(theme: &Theme) -> Color {
    theme.palette().text.scale_alpha(0.55)
}

/// Error and overdue colour.
pub fn danger_color(theme: &Theme) -> Color {
    theme.palette().danger
}

/// The background moved `amount` (0–1) towards the text colour: a neutral
/// fill that works the same way in light and dark.
pub fn tint(theme: &Theme, amount: f32) -> Color {
    let p = theme.palette();
    mix(p.background, p.text, amount)
}

/// Fill for cards and hovered rows.
pub fn soft_fill(theme: &Theme) -> Color {
    tint(theme, 0.04)
}

/// Fill for inputs, chips and secondary buttons (visible on a card too).
fn field_fill(theme: &Theme) -> Color {
    tint(theme, 0.075)
}

// ------------------------------------------------------------ text styles

/// Secondary text.
pub fn muted(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(muted_color(theme)),
    }
}

/// Error or overdue text.
pub fn danger(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(danger_color(theme)),
    }
}

/// Accent-coloured text. Use rarely.
pub fn accent(_theme: &Theme) -> text::Style {
    text::Style {
        color: Some(accent_color()),
    }
}

/// A view's title.
pub fn title<'a>(label: impl text::IntoFragment<'a>) -> text::Text<'a, Theme> {
    text(label).size(TEXT_TITLE)
}

/// A small muted heading above a group of items.
pub fn section_heading<'a>(label: impl text::IntoFragment<'a>) -> text::Text<'a, Theme> {
    text(label).size(TEXT_SMALL).style(muted)
}

// ------------------------------------------------------------- containers

/// A softly filled rounded panel with no border.
pub fn card(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(soft_fill(theme).into()),
        border: border::rounded(RADIUS),
        ..container::Style::default()
    }
}

/// A small pill, e.g. a project tag.
pub fn chip(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(field_fill(theme).into()),
        text_color: Some(muted_color(theme)),
        border: border::rounded(999),
        ..container::Style::default()
    }
}

/// Padding to use with [`chip`].
pub fn chip_padding() -> Padding {
    Padding::from([2, 8])
}

// ---------------------------------------------------------------- buttons

fn button_base(background: Option<Color>, text_color: Color) -> button::Style {
    button::Style {
        background: background.map(Background::Color),
        text_color,
        border: border::rounded(RADIUS),
        ..button::Style::default()
    }
}

fn faded(style: button::Style) -> button::Style {
    button::Style {
        background: style.background.map(|b| b.scale_alpha(0.5)),
        text_color: style.text_color.scale_alpha(0.5),
        ..style
    }
}

/// The one main action on screen: accent fill, white text.
pub fn primary_button(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let base = button_base(Some(accent_color()), Color::WHITE);
    match status {
        button::Status::Active => base,
        button::Status::Hovered | button::Status::Pressed => {
            base.with_background(palette.primary.strong.color)
        }
        button::Status::Disabled => faded(base),
    }
}

/// Other actions: soft neutral fill.
pub fn secondary_button(theme: &Theme, status: button::Status) -> button::Style {
    let base = button_base(Some(field_fill(theme)), theme.palette().text);
    match status {
        button::Status::Active => base,
        button::Status::Hovered | button::Status::Pressed => {
            base.with_background(tint(theme, 0.12))
        }
        button::Status::Disabled => faded(base),
    }
}

/// A link-like button: muted text, no fill; full text colour on hover.
pub fn text_button(theme: &Theme, status: button::Status) -> button::Style {
    let base = button_base(None, muted_color(theme));
    match status {
        button::Status::Active => base,
        button::Status::Hovered | button::Status::Pressed => button::Style {
            text_color: theme.palette().text,
            ..base
        },
        button::Status::Disabled => faded(base),
    }
}

/// A destructive action shown as red text; a faint red fill on hover.
pub fn danger_button(theme: &Theme, status: button::Status) -> button::Style {
    let red = danger_color(theme);
    let base = button_base(None, red);
    match status {
        button::Status::Active => base,
        button::Status::Hovered | button::Status::Pressed => {
            base.with_background(red.scale_alpha(0.12))
        }
        button::Status::Disabled => faded(base),
    }
}

// ----------------------------------------------------------------- inputs

/// A text input with a soft fill, no border, and an accent ring on focus.
pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let palette = theme.extended_palette();
    let ring = match status {
        text_input::Status::Focused { .. } => accent_color(),
        text_input::Status::Hovered => tint(theme, 0.15),
        _ => Color::TRANSPARENT,
    };
    text_input::Style {
        background: Background::Color(field_fill(theme)),
        border: Border {
            radius: RADIUS.into(),
            width: 1.0,
            color: ring,
        },
        icon: muted_color(theme),
        placeholder: muted_color(theme),
        value: theme.palette().text,
        selection: palette.primary.weak.color,
    }
}

/// The [`input`] look for a multi-line `text_editor`.
pub fn editor(theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let as_input = match status {
        text_editor::Status::Active => text_input::Status::Active,
        text_editor::Status::Hovered => text_input::Status::Hovered,
        text_editor::Status::Focused { is_hovered } => text_input::Status::Focused { is_hovered },
        text_editor::Status::Disabled => text_input::Status::Disabled,
    };
    let s = input(theme, as_input);
    text_editor::Style {
        background: s.background,
        border: s.border,
        placeholder: s.placeholder,
        value: s.value,
        selection: s.selection,
    }
}

// ------------------------------------------------------- round 2: navigation

/// The slim bar across the top, in the scheme of the Week tab's title
/// bars (the Weekly Summary's and each day column's): a stronger fill than
/// the page, a hairline below.
pub fn top_bar(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(tint(theme, 0.08).into()),
        border: Border {
            width: 0.0,
            ..Border::default()
        },
        shadow: iced::Shadow {
            color: tint(theme, 0.16),
            offset: iced::Vector::new(0.0, 1.0),
            blur_radius: 0.0,
        },
        ..container::Style::default()
    }
}

/// The pill that holds the tab buttons.
pub fn tab_bar(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(field_fill(theme).into()),
        border: border::rounded(RADIUS),
        ..container::Style::default()
    }
}

/// [`tab_bar`] on the [`top_bar`]: a step lighter than the bar so the pill
/// still shows on its stronger fill.
pub fn top_tab_bar(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(tint(theme, 0.14).into()),
        ..tab_bar(theme)
    }
}

/// A tab inside [`tab_bar`]: the active one is filled with the accent and
/// has white text, like every other selected thing (a calendar day, a ticked
/// checkbox, the primary button); the others are muted text. (The top bar
/// draws all its tabs inactive and slides a [`tab_pill`] under the selected
/// one.)
pub fn tab_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let p = theme.palette();
        if active {
            let pill = tab_pill();
            button::Style {
                background: Some(pill.color.into()),
                text_color: Color::WHITE,
                border: border::rounded(pill.radius),
                shadow: pill.shadow,
                ..button::Style::default()
            }
        } else {
            let text_color = match status {
                button::Status::Hovered | button::Status::Pressed => p.text,
                _ => muted_color(theme),
            };
            button::Style {
                text_color,
                border: border::rounded(RADIUS - 2.0),
                ..button::Style::default()
            }
        }
    }
}

/// The fill of the active [`tab_button`], for a pill that slides between
/// tabs (`motion::pill`): the accent, with a slight shadow.
pub fn tab_pill() -> crate::motion::PillStyle {
    crate::motion::PillStyle {
        color: accent_color(),
        radius: RADIUS - 2.0,
        shadow: iced::Shadow {
            color: Color::BLACK.scale_alpha(0.12),
            offset: iced::Vector::new(0.0, 1.0),
            blur_radius: 2.0,
        },
    }
}

/// Padding for a [`tab_button`].
pub fn tab_padding() -> Padding {
    Padding::from([5, 14])
}

/// The icons the app draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Gear,
    Lock,
    Unlock,
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    Close,
    Plus,
    Warning,
    Copy,
    Check,
    Trash,
    ListBullet,
    ListDash,
    ListNumber,
    Link,
    Search,
}

impl Icon {
    fn bytes(self) -> &'static [u8] {
        match self {
            Icon::Gear => include_bytes!("../../../assets/icons/gear.svg"),
            Icon::Lock => include_bytes!("../../../assets/icons/lock.svg"),
            Icon::Unlock => include_bytes!("../../../assets/icons/unlock.svg"),
            Icon::ChevronLeft => include_bytes!("../../../assets/icons/chevron-left.svg"),
            Icon::ChevronRight => include_bytes!("../../../assets/icons/chevron-right.svg"),
            Icon::ChevronDown => include_bytes!("../../../assets/icons/chevron-down.svg"),
            Icon::Close => include_bytes!("../../../assets/icons/close.svg"),
            Icon::Plus => include_bytes!("../../../assets/icons/plus.svg"),
            Icon::Warning => include_bytes!("../../../assets/icons/warning.svg"),
            Icon::Copy => include_bytes!("../../../assets/icons/copy.svg"),
            Icon::Check => include_bytes!("../../../assets/icons/check.svg"),
            Icon::Trash => include_bytes!("../../../assets/icons/trash.svg"),
            Icon::ListBullet => include_bytes!("../../../assets/icons/list-bullet.svg"),
            Icon::ListDash => include_bytes!("../../../assets/icons/list-dash.svg"),
            Icon::ListNumber => include_bytes!("../../../assets/icons/list-number.svg"),
            Icon::Link => include_bytes!("../../../assets/icons/link.svg"),
            Icon::Search => include_bytes!("../../../assets/icons/search.svg"),
        }
    }
}

/// An 18 px icon in the muted text colour (full text colour on hover when
/// inside a button).
pub fn icon<'a>(which: Icon) -> svg::Svg<'a, Theme> {
    svg(svg::Handle::from_memory(which.bytes()))
        .width(18)
        .height(18)
        .style(|theme: &Theme, status| svg::Style {
            color: Some(match status {
                svg::Status::Hovered => theme.palette().text,
                svg::Status::Idle => muted_color(theme),
            }),
        })
}

/// The arrow of a section that folds: a chevron pointing right when
/// closed, down when open, in the colour of the text around it.
pub fn disclosure<'a>(open: bool) -> svg::Svg<'a, Theme> {
    icon(if open {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    })
    .width(16)
    .height(16)
}

/// A pick list with the app's chevron as its arrow: `list` is built with
/// `pick_list::Handle::None` and [`DROPDOWN_PADDING`], and the chevron is
/// drawn over its right end (it takes no clicks, so they reach the list).
/// Its menu opens below it whenever it fits there ([`crate::open_below`]).
/// iced's own arrow is a small font glyph that doesn't match the other
/// chevrons.
pub fn dropdown<'a, M: 'a>(list: impl Into<Element<'a, M>>) -> Element<'a, M> {
    let chevron = icon(Icon::ChevronDown)
        .width(16)
        .height(16)
        .style(|theme: &Theme, _| svg::Style {
            color: Some(muted_color(theme)),
        });
    // The list itself is wrapped: a stack hands its menu up in a group
    // that `open_below` can't move.
    stack![
        crate::open_below::open_below(list),
        container(chevron)
            .align_right(iced::Fill)
            .center_y(iced::Fill)
            .padding(Padding::ZERO.right(12))
    ]
    .into()
}

/// Padding for a pick list shown with [`dropdown`]: room at the right for
/// the chevron.
pub const DROPDOWN_PADDING: Padding = Padding {
    top: 8.0,
    right: 36.0,
    bottom: 8.0,
    left: 12.0,
};

/// A square icon button: no fill until hovered.
pub fn icon_button(theme: &Theme, status: button::Status) -> button::Style {
    let base = button_base(None, muted_color(theme));
    match status {
        button::Status::Active => base,
        button::Status::Hovered | button::Status::Pressed => button::Style {
            text_color: theme.palette().text,
            ..base.with_background(soft_fill(theme))
        },
        button::Status::Disabled => faded(base),
    }
}

/// Padding for an [`icon_button`].
pub const ICON_PADDING: f32 = 6.0;

// --------------------------------------------------------- round 2: calendar

/// A column of the Week tab. Today's column has an accent ring.
pub fn day_column(is_today: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| container::Style {
        background: Some(soft_fill(theme).into()),
        border: Border {
            radius: RADIUS.into(),
            width: if is_today { 1.5 } else { 0.0 },
            color: accent_color(),
        },
        ..container::Style::default()
    }
}

/// What kind of day a Month cell is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell {
    pub today: bool,
    /// A non-working day: muted.
    pub weekend: bool,
    /// A day of the previous or next month: faded.
    pub outside: bool,
}

/// How strong a Month cell's fill is (a [`tint`] amount): working days
/// stand out from weekends, a little more in light mode, where the soft
/// card fill is faint; days outside the month have none.
pub fn calendar_cell_tint(theme: &Theme, cell: Cell) -> f32 {
    if cell.outside {
        0.0
    } else if cell.weekend {
        0.02
    } else if theme.extended_palette().is_dark {
        0.04
    } else {
        0.065
    }
}

/// A Month grid cell: its [`calendar_cell_tint`] fill, with an accent
/// ring on today.
pub fn calendar_cell(cell: Cell) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let fill = (!cell.outside).then(|| tint(theme, calendar_cell_tint(theme, cell)));
        container::Style {
            background: fill.map(Background::Color),
            text_color: (cell.outside || cell.weekend).then(|| muted_color(theme)),
            border: Border {
                radius: (RADIUS - 2.0).into(),
                width: if cell.today { CELL_RING } else { 0.0 },
                color: accent_color(),
            },
            ..container::Style::default()
        }
    }
}

/// Width of today's accent ring on a Month cell.
pub const CELL_RING: f32 = 1.5;

/// The band across the top of a Month cell holding the date and the
/// completion badge: a shade darker than the cell, rounded at the top
/// only (`radius`). None outside the month, where the cell has no fill.
pub fn calendar_cell_band(cell: Cell, radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |theme| container::Style {
        background: (!cell.outside)
            .then(|| tint(theme, calendar_cell_tint(theme, cell) + 0.035).into()),
        border: border::rounded(border::top(radius)),
        ..container::Style::default()
    }
}

/// A load bar's track (`progress_bar(0.0..=1.0, share).style(theme::load_bar)`):
/// the gray track is the day's capacity, the fill the estimated tasks'
/// share of it in a gray that stands out from the track (lighter in dark
/// mode). [`load_bar_done`] goes on top.
pub fn load_bar(theme: &Theme) -> progress_bar::Style {
    progress_bar::Style {
        background: Background::Color(tint(theme, 0.10)),
        bar: Background::Color(tint(theme, 0.30)),
        border: border::rounded(999),
    }
}

/// The done part of a load bar, stacked over [`load_bar`]: accent on a
/// clear track.
pub fn load_bar_done(_theme: &Theme) -> progress_bar::Style {
    progress_bar::Style {
        background: Background::Color(Color::TRANSPARENT),
        bar: Background::Color(accent_color()),
        border: border::rounded(999),
    }
}

/// How dark the layer behind an overlay is (`motion::dim`).
pub const BACKDROP: f32 = 0.45;

/// The task dialog's: darker than Settings' so the page behind recedes.
/// (iced 0.14 has no backdrop blur.)
pub const DIALOG_BACKDROP: f32 = 0.55;

/// The panel of an overlay: the plain background, rounded, with a shadow.
pub fn overlay_panel(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(theme.palette().background.into()),
        border: border::rounded(RADIUS * 1.5),
        shadow: iced::Shadow {
            color: Color::BLACK.scale_alpha(0.3),
            offset: iced::Vector::new(0.0, 8.0),
            blur_radius: 24.0,
        },
        ..container::Style::default()
    }
}

/// The colour of a priority marker (`!` or `!!`): the accent, `None` for
/// normal priority.
fn priority_color(priority: Priority) -> Option<Color> {
    (priority > 0).then(accent_color)
}

/// The marks after a title for `priority`: a non-breaking space then `!`
/// (Medium) or `!!` (High, and old data's 3); empty for Low (0).
pub fn priority_marks(priority: Priority) -> String {
    match priority.min(2) {
        0 => String::new(),
        level => format!("\u{a0}{}", "!".repeat(usize::from(level))),
    }
}

/// A task title followed by its priority marks ("Write report !!",
/// [`priority_marks`]) in the priority colour, the same on every tab;
/// `priority` 0 shows the title alone. The non-breaking space keeps the
/// marks on the last word's line. Rich text spans only take fixed colours
/// and both colours here come from the theme, so two copies of the same
/// text are stacked (same text, same layout): one draws the title with the
/// marks hidden, the other only the marks. `build` sets size, width,
/// wrapping and so on, alike for both. `struck` draws a line through the
/// title (not the marks), in the title's colour.
pub fn title_with_marks<'a, M: 'a>(
    title: impl text::IntoFragment<'a>,
    priority: Priority,
    struck: bool,
    title_style: impl Fn(&Theme) -> text::Style + Clone + 'a,
    build: impl Fn(text::Rich<'a, (), M>) -> text::Rich<'a, (), M>,
) -> Element<'a, M> {
    let title = title.into_fragment();
    let marks = priority_marks(priority);
    if marks.is_empty() {
        let title = span(title).strikethrough(struck);
        return build(rich_text([title]).style(title_style)).into();
    }
    let hidden = Some(Color::TRANSPARENT);
    let title_layer = rich_text([
        span(title.clone()).strikethrough(struck),
        span(marks.clone()).color_maybe(hidden),
    ])
    .style(title_style);
    let marks_layer =
        rich_text([span(title).color_maybe(hidden), span(marks)]).style(move |_: &Theme| {
            text::Style {
                color: priority_color(priority),
            }
        });
    stack![build(title_layer), build(marks_layer)].into()
}

/// A tooltip's bubble: small text on a [`card`].
pub fn tip<'a, M: 'a>(label: impl text::IntoFragment<'a>) -> container::Container<'a, M, Theme> {
    container(text(label).size(TEXT_SMALL))
        .padding([4, 8])
        .style(card)
}

/// A due/overdue/muted label colour helper for text: `theme::danger` when
/// `overdue`, otherwise `theme::muted`.
pub fn due_style(overdue: bool) -> fn(&Theme) -> text::Style {
    if overdue { danger } else { muted }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_choice_and_system_appearance() {
        assert!(!is_dark(Appearance::System, None));
        assert!(!is_dark(Appearance::System, Some(false)));
        assert!(is_dark(Appearance::System, Some(true)));
        assert!(is_dark(Appearance::Dark, Some(false)));
        assert!(!is_dark(Appearance::Light, Some(true)));
    }

    #[test]
    fn themes_use_the_accent_and_report_their_mode() {
        let light = theme_for(false);
        let dark = theme_for(true);
        assert_eq!(light.palette().primary, accent_color());
        assert_eq!(dark.palette().primary, accent_color());
        assert!(!light.extended_palette().is_dark);
        assert!(dark.extended_palette().is_dark);
    }
}
