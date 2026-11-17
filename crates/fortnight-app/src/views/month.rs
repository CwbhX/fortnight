//! The Month tab: a calendar grid for seeing and navigating. Owner: agent O
//! (round 2).
//!
//! STEP 1 PLACEHOLDER: one row per week with a clickable week label and a
//! clickable cell per day showing the day number and a task count. Agent O
//! builds the finished grid per PLAN.md section 4.2.

use crate::theme;
use fortnight_core::domain::{Task, WeekRef};
use fortnight_core::week::month_weeks;
use iced::widget::{Column, button, column, container, row, scrollable, text};
use iced::{Element, Fill};
use jiff::civil::{Date, Weekday};

#[derive(Debug, Clone)]
pub enum Message {
    DayClicked(Date),
    WeekClicked(WeekRef),
}

/// What `main.rs` must do. FROZEN contract (round 2).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Show this day in the Today tab.
    OpenDay(Date),
    /// Show this week in the Week tab.
    OpenWeek(WeekRef),
}

/// Everything the tab shows, supplied by `main.rs`. FROZEN contract
/// (round 2).
#[derive(Debug, Clone, PartialEq)]
pub struct MonthData {
    /// The first day of the month shown.
    pub month: Date,
    pub today: Date,
    pub week_start: Weekday,
    /// Days that are not working days are muted.
    pub working_days: Vec<Weekday>,
    /// Every task; the view places each by `Task::day()`.
    pub tasks: Vec<Task>,
    /// Start dates of the weeks that have a ready summary (the dot on the
    /// week label).
    pub summary_weeks: Vec<Date>,
}

#[derive(Debug, Default)]
pub struct MonthView {
    data: Option<MonthData>,
}

impl MonthView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces what the view shows. Called after every message: return
    /// early if `data` is unchanged.
    pub fn load(&mut self, data: MonthData) {
        if self.data.as_ref() == Some(&data) {
            return;
        }
        self.data = Some(data);
    }

    pub fn update(&mut self, message: Message) -> Action {
        match message {
            Message::DayClicked(d) => Action::OpenDay(d),
            Message::WeekClicked(w) => Action::OpenWeek(w),
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let Some(data) = &self.data else {
            return text("Loading…").style(theme::muted).into();
        };
        let mut grid = Column::new().spacing(4);
        for week in month_weeks(data.month, data.week_start) {
            let number = week
                .start
                .saturating_add(jiff::ToSpan::days(3))
                .iso_week_date();
            let mut line = row![
                button(text(format!("W{}", number.week())).size(theme::TEXT_SMALL))
                    .style(theme::text_button)
                    .on_press(Message::WeekClicked(week))
            ]
            .spacing(4);
            for day in week.days() {
                let count = data.tasks.iter().filter(|t| t.day() == Some(day)).count();
                let cell = theme::Cell {
                    today: day == data.today,
                    weekend: !data.working_days.contains(&day.weekday()),
                    outside: day.month() != data.month.month(),
                };
                let label = if count > 0 {
                    format!("{}  · {count}", day.day())
                } else {
                    day.day().to_string()
                };
                line = line.push(
                    button(
                        container(text(label).size(theme::TEXT_SMALL))
                            .padding(6)
                            .width(Fill)
                            .height(64)
                            .style(theme::calendar_cell(cell)),
                    )
                    .padding(0)
                    .width(Fill)
                    .style(theme::row_button)
                    .on_press(Message::DayClicked(day)),
                );
            }
            grid = grid.push(line);
        }
        scrollable(column![grid].padding(theme::SPACING * 2.0))
            .height(Fill)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    #[test]
    fn clicks_map_to_actions() {
        let mut v = MonthView::new();
        let d = date(2026, 11, 17);
        assert_eq!(v.update(Message::DayClicked(d)), Action::OpenDay(d));
        let w = WeekRef::new(date(2026, 11, 16));
        assert_eq!(v.update(Message::WeekClicked(w)), Action::OpenWeek(w));
    }
}
