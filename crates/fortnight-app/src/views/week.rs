//! The Week tab: a column per working day, then the report panel. Owner:
//! agent K (round 2).
//!
//! STEP 1 PLACEHOLDER: plain day columns (working days, plus a weekend
//! column when Saturday or Sunday has tasks) with checkboxes, and the report
//! panel below. Agent K adds load bars and the finished layout per PLAN.md
//! section 4.2.

use crate::theme;
use crate::views::report_panel::{self, ReportPanel, ReportPanelData};
use fortnight_core::domain::{Task, TaskId, TaskStatus, WeekRef};
use fortnight_core::report::DayLoad;
use iced::widget::{Column, button, checkbox, column, container, row, scrollable, text};
use iced::{Element, Fill};
use jiff::civil::{Date, Weekday};

#[derive(Debug, Clone)]
pub enum Message {
    /// A task's checkbox.
    Toggle(TaskId, bool),
    /// A day's heading was clicked.
    OpenDay(Date),
    Panel(report_panel::Message),
}

/// What `main.rs` must do. FROZEN contract (round 2).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    SetStatus(TaskId, TaskStatus),
    /// Show this day in the Today tab.
    OpenDay(Date),
    /// Something the embedded report panel asked for.
    Panel(report_panel::Action),
}

/// Everything the tab shows, supplied by `main.rs`. FROZEN contract
/// (round 2).
#[derive(Debug, Clone, PartialEq)]
pub struct WeekData {
    pub week: WeekRef,
    pub today: Date,
    /// Every task; the view places each by `Task::day()`.
    pub tasks: Vec<Task>,
    /// Which days get a column (always shown, in week order). Other days
    /// get one shared "Weekend" column, only when they have tasks.
    pub working_days: Vec<Weekday>,
    /// One per day of the week, in order (seven entries).
    pub loads: Vec<DayLoad>,
    pub panel: ReportPanelData,
}

#[derive(Debug, Default)]
pub struct WeekView {
    data: Option<WeekData>,
    panel: ReportPanel,
}

impl WeekView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces what the view shows. `main.rs` calls this after EVERY
    /// message (and on the minute tick and each streamed piece of text), so
    /// it must be cheap: if `data` equals what the view already has, return
    /// early. Passes `data.panel` on to the embedded panel.
    pub fn load(&mut self, data: WeekData) {
        if self.data.as_ref() == Some(&data) {
            return;
        }
        self.panel.load(data.panel.clone());
        self.data = Some(data);
    }

    pub fn update(&mut self, message: Message) -> Action {
        match message {
            Message::Toggle(id, done) => Action::SetStatus(
                id,
                if done {
                    TaskStatus::Done
                } else {
                    TaskStatus::Todo
                },
            ),
            Message::OpenDay(day) => Action::OpenDay(day),
            Message::Panel(m) => match self.panel.update(m) {
                report_panel::Action::None => Action::None,
                action => Action::Panel(action),
            },
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let Some(data) = &self.data else {
            return text("Loading…").style(theme::muted).into();
        };
        let mut columns = row![].spacing(theme::SPACING);
        for day in data.week.days() {
            if data.working_days.contains(&day.weekday()) {
                columns =
                    columns.push(day_column(data, &[day], day.strftime("%a %-d").to_string()));
            }
        }
        let weekend: Vec<Date> = data
            .week
            .days()
            .into_iter()
            .filter(|d| !data.working_days.contains(&d.weekday()))
            .collect();
        if weekend
            .iter()
            .any(|d| !tasks_on(&data.tasks, *d).is_empty())
        {
            columns = columns.push(day_column(data, &weekend, "Weekend".into()));
        }
        let body = column![
            columns,
            container(self.panel.view().map(Message::Panel))
                .padding(theme::SPACING * 1.5)
                .width(Fill)
                .style(theme::card),
        ]
        .spacing(theme::SPACING * 2.0)
        .padding(theme::SPACING * 2.0);
        scrollable(body).height(Fill).into()
    }
}

/// Tasks whose `Task::day()` is `day`: open first, then done; by position.
pub fn tasks_on(tasks: &[Task], day: Date) -> Vec<&Task> {
    let mut out: Vec<&Task> = tasks.iter().filter(|t| t.day() == Some(day)).collect();
    out.sort_by(|a, b| {
        (!a.status.is_open())
            .cmp(&!b.status.is_open())
            .then(a.position.total_cmp(&b.position))
            .then(a.id.cmp(&b.id))
    });
    out
}

fn day_column<'a>(data: &'a WeekData, days: &[Date], label: String) -> Element<'a, Message> {
    let is_today = days.contains(&data.today);
    let heading = button(text(label).size(theme::TEXT_SMALL).style(if is_today {
        theme::accent
    } else {
        theme::muted
    }))
    .padding(0)
    .style(theme::text_button)
    .on_press(Message::OpenDay(days[0]));
    let mut col = Column::new().spacing(6).push(heading);
    for day in days {
        for task in tasks_on(&data.tasks, *day) {
            let id = task.id;
            col = col.push(
                checkbox(task.is_done())
                    .label(task.title.as_str())
                    .text_size(theme::TEXT_SMALL)
                    .on_toggle(move |b| Message::Toggle(id, b)),
            );
        }
    }
    container(col)
        .padding(theme::SPACING)
        .width(Fill)
        .height(180)
        .style(theme::day_column(is_today))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn task(id: TaskId, day: Option<Date>, status: TaskStatus) -> Task {
        Task {
            id,
            title: format!("t{id}"),
            status,
            due_on: day,
            position: id as f64,
            ..Task::default()
        }
    }

    fn data(tasks: Vec<Task>) -> WeekData {
        let week = WeekRef::new(date(2026, 11, 16));
        WeekData {
            week,
            today: date(2026, 11, 18),
            tasks,
            working_days: vec![
                Weekday::Monday,
                Weekday::Tuesday,
                Weekday::Wednesday,
                Weekday::Thursday,
                Weekday::Friday,
            ],
            loads: Vec::new(),
            panel: ReportPanelData {
                week,
                week_label: "Week 47".into(),
                slots: Vec::new(),
            },
        }
    }

    #[test]
    fn tasks_on_uses_planned_day_and_puts_open_first() {
        let d = date(2026, 11, 18);
        let mut planned = task(3, Some(date(2026, 11, 25)), TaskStatus::Todo);
        planned.planned_for = Some(d);
        let tasks = vec![
            task(1, Some(d), TaskStatus::Done),
            task(2, Some(d), TaskStatus::Todo),
            planned,
            task(4, Some(date(2026, 11, 19)), TaskStatus::Todo),
        ];
        let ids: Vec<_> = tasks_on(&tasks, d).iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![2, 3, 1]);
    }

    #[test]
    fn update_maps_messages_to_actions() {
        let mut v = WeekView::new();
        v.load(data(vec![]));
        assert_eq!(
            v.update(Message::Toggle(4, true)),
            Action::SetStatus(4, TaskStatus::Done)
        );
        assert_eq!(
            v.update(Message::OpenDay(date(2026, 11, 17))),
            Action::OpenDay(date(2026, 11, 17))
        );
    }
}
