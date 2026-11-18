//! The pieces the Week and Today tabs share, so both read the same way:
//! panels with a tinted title bar over their contents (some fold), the
//! day's load bar, and the raised task-card fill.

use crate::{motion, theme};
use fortnight_core::report::{ColumnLoad, fmt_hours};
use iced::widget::{column, container, progress_bar, row, stack, tooltip};
use iced::{Center, Element, Fill, Theme};

/// A title bar (the Work Summary's, each day column's, each Today panel's):
/// a stronger fill than the card, rounded at the top only (`radius`), with
/// a hairline under it.
pub fn title_bar(radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |theme| title_bar_style(theme, radius, false)
}

/// A title bar with nothing under it (a collapsed panel): rounded all
/// round, no hairline.
pub fn title_bar_alone(radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |theme| title_bar_style(theme, radius, true)
}

fn title_bar_style(theme: &Theme, radius: f32, alone: bool) -> container::Style {
    container::Style {
        background: Some(theme::tint(theme, 0.08).into()),
        border: if alone {
            iced::border::rounded(radius)
        } else {
            iced::border::rounded(iced::border::top(radius))
        },
        shadow: if alone {
            iced::Shadow::default()
        } else {
            iced::Shadow {
                color: theme::tint(theme, 0.16),
                offset: iced::Vector::new(0.0, 1.0),
                blur_radius: 0.0,
            }
        },
        ..container::Style::default()
    }
}

/// Padding inside a panel's title bar.
pub fn title_padding() -> iced::Padding {
    iced::Padding::from([theme::SPACING * 0.6, theme::SPACING])
}

/// A panel: `head` in a title bar over `body` (none: the bar alone), on
/// the soft card fill.
pub fn panel<'a, M: 'a>(head: Element<'a, M>, body: Option<Element<'a, M>>) -> Element<'a, M> {
    let bar = container(head).padding(title_padding()).width(Fill);
    let content: Element<'a, M> = match body {
        Some(body) => column![
            bar.style(title_bar(theme::RADIUS)),
            container(body).padding(theme::SPACING).width(Fill)
        ]
        .into(),
        None => bar.style(title_bar_alone(theme::RADIUS)).into(),
    };
    container(content).width(Fill).style(theme::card).into()
}

/// A [`panel`] that folds: its body unfolds when `open` turns true and
/// folds away when it turns false (`motion::fold`), so it is given either
/// way. `key` tells the folding panels of a page apart, so one taking
/// another's place doesn't unfold or fold.
pub fn folding_panel<'a, M: 'a>(
    head: Element<'a, M>,
    open: bool,
    body: Element<'a, M>,
    key: u64,
) -> Element<'a, M> {
    let bar = container(head).padding(title_padding()).width(Fill);
    let bar = if open {
        bar.style(title_bar(theme::RADIUS))
    } else {
        bar.style(title_bar_alone(theme::RADIUS))
    };
    let body = container(body).padding(theme::SPACING).width(Fill);
    container(column![bar, motion::fold(open, body).key(key)])
        .width(Fill)
        .style(theme::card)
        .into()
}

/// The load bar in a day's title bar. The track is the day's capacity.
/// Tasks with an estimate take their estimate; tasks without one share
/// what is left of the day evenly (none left: they take nothing). Done
/// tasks' parts are the accent.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadLine {
    /// Everything planned (open and done) over capacity, 0–1.
    pub fraction: f32,
    /// The done part over capacity, 0–1 (at most `fraction`).
    pub done_fraction: f32,
    /// Estimates booked beyond capacity, when there are any.
    pub over_by: Option<u32>,
    /// The figures for the bar's tooltip, e.g. "4h of 8h estimated · 2
    /// without a time: 2h each · 3h done".
    pub figures: String,
}

/// The bar's figures for a column's (or day's) load.
pub fn load_line(load: &ColumnLoad) -> LoadLine {
    let mut figures = vec![format!(
        "{} of {} estimated",
        fmt_hours(load.estimated_minutes),
        fmt_hours(load.capacity_minutes)
    )];
    if load.untimed > 0 {
        let each = if load.untimed_share >= 1.0 {
            format!("{} each", fmt_hours(load.untimed_share.round() as u32))
        } else {
            "no time left".to_string()
        };
        figures.push(format!("{} without a time: {each}", load.untimed));
    }
    if load.done_minutes > 0.0 {
        figures.push(format!(
            "{} done",
            fmt_hours(load.done_minutes.round() as u32)
        ));
    }
    if let Some(over) = load.over_by {
        figures.push(format!("over by {}", fmt_hours(over)));
    }
    LoadLine {
        fraction: load.fraction(),
        done_fraction: load.done_fraction(),
        over_by: load.over_by,
        figures: figures.join(" · "),
    }
}

/// The load bar, with the figures in its tooltip and a warning sign after
/// it when over capacity.
pub fn load_view<'a, M: 'a>(line: &LoadLine) -> Element<'a, M> {
    // The track with the whole share, then the done part on top.
    let bar = stack![
        progress_bar(0.0..=1.0, line.fraction)
            .girth(6)
            .style(theme::load_bar),
        progress_bar(0.0..=1.0, line.done_fraction)
            .girth(6)
            .style(theme::load_bar_done),
    ];
    let mut row = row![tooltip(
        bar,
        theme::tip(line.figures.clone()),
        tooltip::Position::Bottom
    )]
    .spacing(6)
    .align_y(Center);
    if line.over_by.is_some() {
        row = row.push(
            theme::icon(theme::Icon::Warning)
                .width(14)
                .height(14)
                .style(|theme: &Theme, _| iced::widget::svg::Style {
                    color: Some(theme.palette().warning),
                }),
        );
    }
    row.into()
}

/// A task card's fill: raised above the panel, quieter when done, lighter
/// when `lit` (hovered or held).
pub fn card_fill(theme: &Theme, done: bool, lit: bool) -> iced::Color {
    theme::tint(
        theme,
        match (done, lit) {
            (true, false) => 0.05,
            (true, true) => 0.075,
            (false, false) => 0.1,
            (false, true) => 0.13,
        },
    )
}

/// A task card's corner radius.
pub const CARD_RADIUS: f32 = 6.0;
