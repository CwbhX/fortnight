//! The top bar (PLAN.md section 4.2): the Today | Week | Month tabs, period
//! navigation over the shared selected date (centred), and Lock and the
//! Settings gear on the right. Clicking the date opens a calendar to pick
//! a day (today is shaded); double-clicking it jumps back to today. (The
//! add-a-task input is in the Today tab, `views/day.rs`.)
//! Owner: orchestrator.
//!
//! ```text
//! [ Today | Week | Month ]          ‹ Nov 16 to Nov 20 ›            lock  gear
//! ```

use crate::calendar::{Popup, month_picker, year_picker};
use crate::motion;
use crate::theme::{self, Icon};
use fortnight_core::domain::Schedule;
use fortnight_core::week::{add_months, month_label, week_containing};
use iced::widget::{button, container, mouse_area, row, text, tooltip};
use iced::{Center, Element, Fill};
use jiff::ToSpan;
use jiff::civil::Date;

/// The three zoom levels over the same tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Today,
    Week,
    Month,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Today, Tab::Week, Tab::Month];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Today => "Today",
            Tab::Week => "Week",
            Tab::Month => "Month",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Tab(Tab),
    /// `‹`: back one day, week or month.
    Prev,
    /// `›`: forward one day, week or month.
    Next,
    /// Jump back to today (`T`, or a double click on the date).
    Today,
    /// A click on the date: open or close the calendar.
    ToggleCalendar,
    /// A click outside the open calendar.
    CloseCalendar,
    /// Show the calendar month before (-1) or after (+1).
    CalendarMonth(i32),
    /// A day picked in the calendar.
    PickDate(Date),
    /// The lock icon: lock when unlocked, unlock prompt when locked.
    Lock,
    /// The gear.
    Settings,
}

/// The lock icon's state; `None` hides it (no key file).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockIcon {
    Locked,
    Unlocked,
}

/// What the bar shows.
pub struct Bar<'a> {
    pub tab: Tab,
    pub selected: Date,
    pub today: Date,
    pub schedule: &'a Schedule,
    pub lock: Option<LockIcon>,
    /// A dot on the Week tab: a report became ready and hasn't been seen.
    pub week_badge: bool,
    /// The month the date calendar shows, while it is open.
    pub calendar: Option<Date>,
}

/// Moves `selected` by one step of `tab` (`dir` is -1 or +1).
pub fn step(tab: Tab, selected: Date, dir: i32) -> Date {
    match tab {
        Tab::Today => selected.saturating_add(i64::from(dir).days()),
        Tab::Week => selected.saturating_add(i64::from(dir * 7).days()),
        Tab::Month => add_months(selected, dir),
    }
}

/// The first and last working day of the week containing `selected` (the
/// whole week if no day is a working day): what the Week tab's label names
/// and its calendar bands.
pub fn week_span(selected: Date, schedule: &Schedule) -> (Date, Date) {
    let week = week_containing(selected, schedule.week_start);
    let days: Vec<Date> = week
        .days()
        .into_iter()
        .filter(|d| schedule.working_days.contains(&d.weekday()))
        .collect();
    match (days.first(), days.last()) {
        (Some(f), Some(l)) => (*f, *l),
        _ => (week.start, week.end()),
    }
}

/// "Mon 16 Nov"; "Nov 16 to Nov 20" (the week's working days);
/// "November 2026". The year is added to day and week labels when it is not
/// `today`'s year.
pub fn period_label(tab: Tab, selected: Date, today: Date, schedule: &Schedule) -> String {
    let year = |d: Date| {
        if d.year() == today.year() {
            String::new()
        } else {
            format!(" {}", d.year())
        }
    };
    match tab {
        Tab::Today => format!("{}{}", selected.strftime("%a %-d %b"), year(selected)),
        Tab::Week => {
            let (first, last) = week_span(selected, schedule);
            format!(
                "{} to {}{}",
                first.strftime("%b %-d"),
                last.strftime("%b %-d"),
                year(last)
            )
        }
        Tab::Month => month_label(selected),
    }
}

pub fn view<'a>(bar: Bar<'a>) -> Element<'a, Message> {
    // Every tab is drawn unselected; the pill, with the same labels in
    // white on it, slides to the selected one (`motion::pill`).
    let mut tabs = row![].spacing(2);
    let mut lit = row![].spacing(2);
    for tab in Tab::ALL {
        let label = if tab == Tab::Week && bar.week_badge {
            format!("{} •", tab.label())
        } else {
            tab.label().to_string()
        };
        lit = lit.push(
            container(
                text(label.clone())
                    .size(theme::TEXT_BODY)
                    .color(iced::Color::WHITE),
            )
            .padding(theme::tab_padding()),
        );
        tabs = tabs.push(
            button(text(label).size(theme::TEXT_BODY))
                .padding(theme::tab_padding())
                .style(theme::tab_button(false))
                .on_press(Message::Tab(tab)),
        );
    }
    let selected = Tab::ALL.iter().position(|t| *t == bar.tab).unwrap_or(0);
    let tabs = container(motion::pill(tabs, lit, selected, theme::tab_pill()))
        .padding(3)
        .style(theme::top_tab_bar);

    let icon_button = |icon: Icon, message: Message, tip: &'a str| {
        tooltip(
            button(theme::icon(icon))
                .padding(theme::ICON_PADDING)
                .style(theme::icon_button)
                .on_press(message),
            theme::tip(tip),
            tooltip::Position::Bottom,
        )
    };

    // The arrows hug the date. A click on the date opens the calendar; a
    // double click jumps to today. (A mouse area, not a button: a button
    // would take the clicks and the double click would never be seen.)
    let date = container(
        text(period_label(bar.tab, bar.selected, bar.today, bar.schedule))
            .size(theme::TEXT_BODY)
            .font(theme::FONT_BOLD)
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .padding([5, 10])
    .style(date_fill(bar.calendar.is_some()));
    let date = mouse_area(date)
        .on_press(Message::ToggleCalendar)
        .on_double_click(Message::Today)
        .interaction(iced::mouse::Interaction::Pointer);
    // Today: a month calendar marking the day. Week: the same, banding the
    // days the label names. Month: the months of the year (the arrows step
    // a year, 12 months).
    let picker = bar.calendar.map(|month| match bar.tab {
        Tab::Month => year_picker(
            month.year(),
            bar.selected,
            bar.today,
            |step| Message::CalendarMonth(step * 12),
            Message::PickDate,
        ),
        Tab::Today | Tab::Week => {
            let (selected, range) = match bar.tab {
                Tab::Week => (None, Some(week_span(bar.selected, bar.schedule))),
                _ => (Some(bar.selected), None),
            };
            month_picker(
                month,
                selected,
                range,
                bar.today,
                bar.schedule.week_start,
                Message::CalendarMonth,
                Message::PickDate,
                None,
            )
        }
    });
    let period = row![
        icon_button(Icon::ChevronLeft, Message::Prev, "Previous (←)"),
        Popup::new(date, picker, Message::CloseCalendar).centered(),
        icon_button(Icon::ChevronRight, Message::Next, "Next (→)"),
    ]
    .spacing(2)
    .align_y(Center);

    let mut right = row![].spacing(2).align_y(Center);
    match bar.lock {
        Some(LockIcon::Unlocked) => {
            right = right.push(icon_button(Icon::Unlock, Message::Lock, "API key unlocked"));
        }
        Some(LockIcon::Locked) => {
            right = right.push(icon_button(Icon::Lock, Message::Lock, "Unlock the API key"));
        }
        None => {}
    }
    right = right.push(icon_button(Icon::Gear, Message::Settings, "Settings (⌘,)"));

    // Equal-width sides keep the date centred in the window.
    let first = row![
        container(tabs).width(Fill),
        period,
        container(right).width(Fill).align_right(Fill),
    ]
    .spacing(theme::SPACING)
    .align_y(Center);

    container(first.padding([theme::SPACING, theme::SPACING * 1.5]))
        .width(Fill)
        .style(theme::top_bar)
        .into()
}

/// The date: plain text with a soft fill while its calendar is open.
fn date_fill(open: bool) -> impl Fn(&iced::Theme) -> container::Style {
    move |theme| container::Style {
        background: open.then(|| theme.palette().text.scale_alpha(0.07).into()),
        border: iced::border::rounded(theme::RADIUS),
        ..container::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    /// Monday 16 November 2026.
    fn today() -> Date {
        date(2026, 11, 16)
    }

    #[test]
    fn steps_move_by_day_week_month() {
        let d = date(2026, 1, 31);
        assert_eq!(step(Tab::Today, d, 1), date(2026, 2, 1));
        assert_eq!(step(Tab::Week, d, -1), date(2026, 1, 24));
        assert_eq!(step(Tab::Month, d, 1), date(2026, 2, 28));
    }

    #[test]
    fn week_span_is_the_working_days_from_the_week_start() {
        let mut s = Schedule::default();
        assert_eq!(
            week_span(date(2026, 11, 19), &s),
            (date(2026, 11, 16), date(2026, 11, 20))
        );
        // Sunday start, Sunday to Thursday working: Sun 22 is in the next
        // week.
        s.week_start = jiff::civil::Weekday::Sunday;
        s.working_days = vec![
            jiff::civil::Weekday::Sunday,
            jiff::civil::Weekday::Monday,
            jiff::civil::Weekday::Tuesday,
            jiff::civil::Weekday::Wednesday,
            jiff::civil::Weekday::Thursday,
        ];
        assert_eq!(
            week_span(date(2026, 11, 19), &s),
            (date(2026, 11, 15), date(2026, 11, 19))
        );
        assert_eq!(
            week_span(date(2026, 11, 22), &s),
            (date(2026, 11, 22), date(2026, 11, 26))
        );
    }

    #[test]
    fn period_labels() {
        let s = Schedule::default();
        assert_eq!(period_label(Tab::Today, today(), today(), &s), "Mon 16 Nov");
        assert_eq!(
            period_label(Tab::Week, date(2026, 11, 19), today(), &s),
            "Nov 16 to Nov 20"
        );
        assert_eq!(
            period_label(Tab::Week, date(2026, 12, 1), today(), &s),
            "Nov 30 to Dec 4"
        );
        assert_eq!(
            period_label(Tab::Today, date(2027, 1, 4), today(), &s),
            "Mon 4 Jan 2027"
        );
        assert_eq!(
            period_label(Tab::Month, today(), today(), &s),
            "November 2026"
        );
    }
}
