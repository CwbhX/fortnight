//! The top bar (PLAN.md section 4.2): the Today | Week | Month tabs, period
//! navigation over the shared selected date (centred), Lock and the
//! Settings gear, and the always-visible quick-add bar under them.
//! Clicking the date opens a calendar to pick a day (today is shaded);
//! double-clicking it jumps back to today.
//! Owner: orchestrator.
//!
//! ```text
//! [ Today | Week | Month ]          ‹ Nov 16 to Nov 20 ›            lock  gear
//! [ Write Q4 report fri #work ~2h !                                              ]
//! ```

use crate::calendar::{Popup, month_picker};
use crate::theme::{self, Icon};
use fortnight_core::domain::{NewTask, Schedule};
use fortnight_core::parse;
use fortnight_core::week::{add_months, month_label, week_containing};
use iced::widget::{button, container, mouse_area, row, text, tooltip};
use iced::{Center, Element, Fill};
use jiff::ToSpan;
use jiff::civil::Date;

/// The id of the Today tab's add-a-task input, for `operation::focus`.
pub const QUICK_ADD_ID: &str = "quick-add";

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
            let week = week_containing(selected, schedule.week_start);
            let days: Vec<Date> = week
                .days()
                .into_iter()
                .filter(|d| schedule.working_days.contains(&d.weekday()))
                .collect();
            let (first, last) = match (days.first(), days.last()) {
                (Some(f), Some(l)) => (*f, *l),
                _ => (week.start, week.end()),
            };
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

/// The task a quick-add line creates, or `None` if it has no title.
/// A date in the line is the day you plan to do it (where the task is
/// shown), not a deadline. In the Today tab a line without a date is
/// planned for the day shown.
pub fn quick_add_task(input: &str, tab: Tab, selected: Date, today: Date) -> Option<NewTask> {
    let parsed = parse::parse_quick_add(input, today);
    if parsed.title.trim().is_empty() {
        return None;
    }
    let planned_for = match tab {
        Tab::Today => parsed.due_on.or(Some(selected)),
        _ => parsed.due_on,
    };
    Some(NewTask {
        title: parsed.title,
        project: parsed.project,
        planned_for,
        estimate_minutes: parsed.estimate_minutes,
        priority: parsed.priority,
        ..NewTask::default()
    })
}

pub fn view<'a>(bar: Bar<'a>) -> Element<'a, Message> {
    let mut tabs = row![].spacing(2);
    for tab in Tab::ALL {
        let label = if tab == Tab::Week && bar.week_badge {
            format!("{} •", tab.label())
        } else {
            tab.label().to_string()
        };
        tabs = tabs.push(
            button(text(label).size(theme::TEXT_BODY))
                .padding(theme::tab_padding())
                .style(theme::tab_button(tab == bar.tab))
                .on_press(Message::Tab(tab)),
        );
    }
    let tabs = container(tabs).padding(3).style(theme::tab_bar);

    let icon_button = |icon: Icon, message: Message, tip: &'a str| {
        tooltip(
            button(theme::icon(icon))
                .padding(theme::ICON_PADDING)
                .style(theme::icon_button)
                .on_press(message),
            container(text(tip).size(theme::TEXT_SMALL))
                .padding([4, 8])
                .style(theme::card),
            tooltip::Position::Bottom,
        )
    };

    // The arrows hug the date. A click on the date opens the calendar; a
    // double click jumps to today. (A mouse area, not a button: a button
    // would take the clicks and the double click would never be seen.)
    let date = container(
        text(period_label(bar.tab, bar.selected, bar.today, bar.schedule))
            .size(theme::TEXT_BODY)
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .padding([5, 10])
    .style(date_fill(bar.calendar.is_some()));
    let date = mouse_area(date)
        .on_press(Message::ToggleCalendar)
        .on_double_click(Message::Today)
        .interaction(iced::mouse::Interaction::Pointer);
    // In the Week tab the calendar bands the week shown instead of marking
    // one day.
    let (selected, range) = match bar.tab {
        Tab::Week => {
            let week = week_containing(bar.selected, bar.schedule.week_start);
            (None, Some((week.start, week.end())))
        }
        _ => (Some(bar.selected), None),
    };
    let picker = bar.calendar.map(|month| {
        month_picker(
            month,
            selected,
            range,
            bar.today,
            Message::CalendarMonth,
            Message::PickDate,
            None,
        )
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

    #[test]
    fn quick_add_dates_undated_tasks_on_the_day_shown() {
        let shown = date(2026, 11, 18);
        let t = quick_add_task("Buy milk #home", Tab::Today, shown, today()).unwrap();
        assert_eq!(t.planned_for, Some(shown));
        assert_eq!(t.due_on, None);
        assert_eq!(t.project.as_deref(), Some("home"));

        let t = quick_add_task("Report fri", Tab::Today, shown, today()).unwrap();
        assert_eq!(t.planned_for, Some(date(2026, 11, 20)));
        assert_eq!(t.due_on, None);

        let t = quick_add_task("Inbox item", Tab::Week, shown, today()).unwrap();
        assert_eq!(t.planned_for, None);

        assert_eq!(quick_add_task("   ", Tab::Today, shown, today()), None);
        assert_eq!(quick_add_task("#work", Tab::Today, shown, today()), None);
    }
}
