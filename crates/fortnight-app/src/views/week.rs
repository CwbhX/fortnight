//! The This-week view: open tasks grouped by day, and the Advice card.
//! Owner: agent H.
//!
//! Left: open tasks in an "Overdue" group, then one group per day of the
//! week (today highlighted), then a count of tasks later or without a date.
//! Right: the Advice card (generating, blocked, empty, failed or ready) and
//! the plan facts behind a "Show the facts" toggle.
//!
//! Skipped for the prototype: estimates and load against capacity per day,
//! Apply buttons for suggestions, and opening Markdown links (clicks are
//! ignored).

use crate::theme;
use fortnight_core::domain::{Report, ReportStatus, Task, TaskId, WeekRef};
use iced::widget::{
    Column, Space, button, checkbox, column, container, markdown, row, scrollable, text,
};
use iced::{Center, Element, Fill, Theme};
use jiff::civil::Date;
use jiff::tz::TimeZone;

/// Maximum width of the two-column layout.
const WIDE_WIDTH: f32 = theme::CONTENT_WIDTH + 360.0;

#[derive(Debug, Clone)]
pub enum Message {
    /// A task's checkbox.
    Toggle(TaskId, bool),
    /// "Generate advice", "Retry" or "Regenerate".
    Generate,
    /// "Show the facts" / "Hide the facts".
    ToggleFacts,
    /// A link in rendered Markdown; ignored.
    LinkClicked(markdown::Uri),
}

/// What `main.rs` must do. Contract: do not change without the orchestrator.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Generate (or regenerate) the advice for the current week.
    Generate,
    SetDone(TaskId, bool),
}

/// Everything the view shows, supplied by `main.rs`.
#[derive(Debug, Clone, PartialEq)]
pub struct WeekData {
    pub week: WeekRef,
    pub week_label: String,
    pub today: Date,
    /// Every task; the view picks the ones to show.
    pub tasks: Vec<Task>,
    /// The deterministic plan facts, as Markdown.
    pub facts_md: String,
    /// The latest advice for the week, if any (may be `Failed`).
    pub advice: Option<Report>,
    pub generating: bool,
    /// Why generating is not possible right now; `None` when it is.
    pub blocked: Option<String>,
}

#[derive(Debug, Default)]
pub struct WeekView {
    data: Option<WeekData>,
    /// Parsed `advice.text()`; reparsed only when that text changes.
    advice_md: markdown::Content,
    advice_src: Option<String>,
    /// Parsed `facts_md`; reparsed only when it changes.
    facts_md: markdown::Content,
    facts_src: String,
    show_facts: bool,
}

impl WeekView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces what the view shows. `main.rs` calls this after EVERY
    /// message (and on the minute tick), so it must be cheap: if `data`
    /// equals what the view already has, return early. It must keep
    /// in-progress UI state (an open editor, typed text) unless the thing it
    /// belongs to changed (a different week or report id).
    pub fn load(&mut self, data: WeekData) {
        if self.data.as_ref() == Some(&data) {
            return;
        }
        if self.data.as_ref().map(|d| d.week) != Some(data.week) {
            self.show_facts = false;
        }
        let advice_text = data
            .advice
            .as_ref()
            .and_then(Report::text)
            .map(str::to_owned);
        if advice_text != self.advice_src {
            self.advice_md = markdown::Content::parse(advice_text.as_deref().unwrap_or(""));
            self.advice_src = advice_text;
        }
        if data.facts_md != self.facts_src {
            self.facts_md = markdown::Content::parse(&data.facts_md);
            self.facts_src = data.facts_md.clone();
        }
        self.data = Some(data);
    }

    pub fn update(&mut self, message: Message) -> Action {
        match message {
            Message::Toggle(id, done) => Action::SetDone(id, done),
            Message::Generate => Action::Generate,
            Message::ToggleFacts => {
                self.show_facts = !self.show_facts;
                Action::None
            }
            Message::LinkClicked(_) => Action::None,
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let Some(data) = &self.data else {
            return container(theme::section_heading("Loading…"))
                .padding(theme::SPACING * 2.0)
                .into();
        };

        let header = column![
            theme::title("This week"),
            row![
                text(&data.week_label).size(theme::TEXT_BODY),
                text(format!("Today is {}", data.today.strftime("%A %-d %B")))
                    .size(theme::TEXT_SMALL)
                    .style(theme::muted),
            ]
            .spacing(theme::SPACING)
            .align_y(Center),
        ]
        .spacing(4);

        let body = row![
            container(self.tasks_column(data)).width(Fill),
            container(self.advice_column(data)).width(Fill),
        ]
        .spacing(theme::SPACING * 2.0);

        scrollable(
            container(
                column![header, body]
                    .spacing(theme::SPACING * 2.0)
                    .padding(theme::SPACING * 2.0)
                    .max_width(WIDE_WIDTH),
            )
            .center_x(Fill),
        )
        .height(Fill)
        .into()
    }

    fn tasks_column<'a>(&'a self, data: &'a WeekData) -> Element<'a, Message> {
        let groups = Groups::new(data);
        let mut col = Column::new().spacing(theme::SPACING * 1.5);

        if !groups.overdue.is_empty() {
            let heading = group_heading(
                text("Overdue").size(theme::TEXT_SMALL).style(theme::danger),
                groups.overdue.len(),
            );
            col =
                col.push(container(group(heading, &groups.overdue)).padding([0.0, theme::SPACING]));
        }

        for (day, tasks) in groups.days {
            let label = day.strftime("%A %-d").to_string();
            let is_today = day == data.today;
            let heading_text = if is_today {
                text(format!("{label} · today"))
                    .size(theme::TEXT_SMALL)
                    .style(theme::accent)
            } else {
                text(label).size(theme::TEXT_SMALL).style(theme::muted)
            };
            let heading = group_heading(heading_text, tasks.len());
            let content = if tasks.is_empty() {
                heading
            } else {
                group(heading, &tasks)
            };
            col = col.push(if is_today {
                container(content)
                    .padding(theme::SPACING)
                    .width(Fill)
                    .style(theme::card)
            } else {
                container(content).padding([0.0, theme::SPACING])
            });
        }

        if groups.later > 0 {
            col = col.push(
                container(
                    text(format!("Later / no date: {}", groups.later))
                        .size(theme::TEXT_SMALL)
                        .style(theme::muted),
                )
                .padding([0.0, theme::SPACING]),
            );
        }
        col.into()
    }

    fn advice_column<'a>(&'a self, data: &'a WeekData) -> Element<'a, Message> {
        let mut card = Column::new()
            .spacing(theme::SPACING)
            .push(text("Advice").size(theme::TEXT_HEADING));

        let blocked_hint = |col: Column<'a, Message>| match &data.blocked {
            Some(why) => col.push(text(why).size(theme::TEXT_SMALL).style(theme::muted)),
            None => col,
        };

        match (&data.advice, data.generating) {
            (_, true) => {
                card = card.push(text("Generating…").style(theme::muted));
            }
            (None, false) => {
                if let Some(why) = &data.blocked {
                    card = card.push(text(why).style(theme::muted));
                } else {
                    card = card
                        .push(
                            text(
                                "Advice for the week ahead is generated automatically \
                                 on the plan day (Monday 08:00 by default). You can \
                                 also generate it now.",
                            )
                            .style(theme::muted),
                        )
                        .push(
                            button(text("Generate advice"))
                                .padding([8, 14])
                                .style(theme::primary_button)
                                .on_press(Message::Generate),
                        );
                }
            }
            (Some(report), false) if report.status == ReportStatus::Failed => {
                let error = report.error.as_deref().unwrap_or("Unknown error");
                card = card
                    .push(text(format!("Generating failed: {error}")).style(theme::danger))
                    .push(
                        button(text("Retry"))
                            .padding([8, 14])
                            .style(theme::primary_button)
                            .on_press_maybe(data.blocked.is_none().then_some(Message::Generate)),
                    );
                card = blocked_hint(card);
            }
            (Some(report), false) => {
                if report.text().is_some_and(|t| !t.trim().is_empty()) {
                    card = card.push(
                        markdown::view(self.advice_md.items(), markdown_settings())
                            .map(Message::LinkClicked),
                    );
                } else {
                    card = card.push(text("The advice is empty.").style(theme::muted));
                }
                card = card.push(
                    row![
                        button(text("Regenerate").size(theme::TEXT_SMALL))
                            .padding([6, 12])
                            .style(theme::secondary_button)
                            .on_press_maybe(data.blocked.is_none().then_some(Message::Generate)),
                        text(report_meta(report))
                            .size(theme::TEXT_SMALL)
                            .style(theme::muted),
                    ]
                    .spacing(theme::SPACING)
                    .align_y(Center),
                );
                card = blocked_hint(card);
            }
        }

        let mut col = Column::new().spacing(theme::SPACING).push(
            container(card)
                .padding(theme::SPACING * 1.5)
                .width(Fill)
                .style(theme::card),
        );

        if !data.facts_md.trim().is_empty() {
            let label = if self.show_facts {
                "Hide the facts"
            } else {
                "Show the facts"
            };
            col = col.push(
                button(text(label).size(theme::TEXT_SMALL))
                    .padding([4, 0])
                    .style(theme::text_button)
                    .on_press(Message::ToggleFacts),
            );
            if self.show_facts {
                col = col.push(
                    container(
                        markdown::view(self.facts_md.items(), markdown_settings())
                            .map(Message::LinkClicked),
                    )
                    .padding(theme::SPACING * 1.5)
                    .width(Fill)
                    .style(theme::card),
                );
            }
        }
        col.into()
    }
}

/// The open tasks of a week, grouped for display.
#[derive(Debug)]
struct Groups<'a> {
    /// Open tasks due before today.
    overdue: Vec<&'a Task>,
    /// One entry per day of the week, in order; open tasks due that day
    /// (and not overdue).
    days: Vec<(Date, Vec<&'a Task>)>,
    /// Open tasks due outside the week (and not overdue), or with no date.
    later: usize,
}

impl<'a> Groups<'a> {
    fn new(data: &'a WeekData) -> Self {
        let mut open: Vec<&Task> = data.tasks.iter().filter(|t| t.status.is_open()).collect();
        open.sort_by(|a, b| {
            a.due_on
                .cmp(&b.due_on)
                .then(a.position.total_cmp(&b.position))
                .then(a.id.cmp(&b.id))
        });

        let mut groups = Groups {
            overdue: Vec::new(),
            days: data
                .week
                .days()
                .into_iter()
                .map(|d| (d, Vec::new()))
                .collect(),
            later: 0,
        };
        for task in open {
            match task.due_on {
                Some(due) if due < data.today => groups.overdue.push(task),
                Some(due) if data.week.contains(due) => {
                    if let Some((_, tasks)) = groups.days.iter_mut().find(|(d, _)| *d == due) {
                        tasks.push(task);
                    }
                }
                _ => groups.later += 1,
            }
        }
        groups
    }
}

fn group_heading<'a>(label: text::Text<'a, Theme>, count: usize) -> Element<'a, Message> {
    // Empty days stay one compact line: "Monday 16  —".
    let count = if count > 0 {
        count.to_string()
    } else {
        "—".to_string()
    };
    row![
        label,
        text(count).size(theme::TEXT_SMALL).style(theme::muted)
    ]
    .spacing(theme::SPACING / 2.0)
    .into()
}

fn group<'a>(heading: Element<'a, Message>, tasks: &[&'a Task]) -> Element<'a, Message> {
    let rows = Column::with_children(tasks.iter().map(|t| task_row(t))).spacing(4);
    column![heading, rows].spacing(theme::SPACING / 2.0).into()
}

fn task_row(task: &Task) -> Element<'_, Message> {
    let id = task.id;
    let mut line = row![
        checkbox(task.is_done()).on_toggle(move |b| Message::Toggle(id, b)),
        text(&task.title).size(theme::TEXT_BODY),
    ]
    .spacing(theme::SPACING / 2.0)
    .align_y(Center);
    if let Some(project) = &task.project {
        line = line.push(Space::new().width(4)).push(
            container(text(format!("#{project}")).size(theme::TEXT_SMALL))
                .padding(theme::chip_padding())
                .style(theme::chip),
        );
    }
    line.into()
}

/// Markdown settings at body size. Only the link colour comes from the
/// palette, and it is the accent in both themes; text colour follows the
/// active theme when drawn.
fn markdown_settings() -> markdown::Settings {
    markdown::Settings::with_text_size(
        theme::TEXT_BODY,
        markdown::Style::from_palette(theme::theme_for(false).palette()),
    )
}

/// "model · Mon 16 Nov 08:00" (local time).
fn report_meta(report: &Report) -> String {
    let created = report
        .created_at
        .to_zoned_datetime(TimeZone::system())
        .strftime("%a %-d %b %H:%M")
        .to_string();
    match &report.model {
        Some(model) => format!("{model} · {created}"),
        None => created,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fortnight_core::domain::{ReportKind, TaskStatus};
    use jiff::civil::date;
    use jiff::{Timestamp, ToSpan};

    fn task(id: TaskId, due: Option<Date>, status: TaskStatus) -> Task {
        Task {
            id,
            title: format!("Task {id}"),
            notes: String::new(),
            status,
            project: None,
            due_on: due,
            position: id as f64,
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::UNIX_EPOCH,
            completed_at: None,
        }
    }

    /// Week of Mon 16 Nov 2026, today Wednesday 18 Nov.
    fn data(tasks: Vec<Task>) -> WeekData {
        WeekData {
            week: WeekRef::new(date(2026, 11, 16)),
            week_label: "Week 47".into(),
            today: date(2026, 11, 18),
            tasks,
            facts_md: "- 3 open tasks".into(),
            advice: None,
            generating: false,
            blocked: None,
        }
    }

    fn report(text: &str) -> Report {
        Report {
            id: 1,
            kind: ReportKind::Plan,
            week_start: date(2026, 11, 16),
            status: ReportStatus::Ready,
            digest_json: "{}".into(),
            content_md: Some(text.into()),
            edited_md: None,
            model: Some("m".into()),
            endpoint_host: None,
            prompt_version: 1,
            error: None,
            created_at: Timestamp::UNIX_EPOCH,
        }
    }

    fn ids(tasks: &[&Task]) -> Vec<TaskId> {
        tasks.iter().map(|t| t.id).collect()
    }

    #[test]
    fn groups_tasks_by_day_with_overdue_first() {
        let d = |n: i64| Some(date(2026, 11, 16).saturating_add(n.days()));
        let data = data(vec![
            task(1, d(-3), TaskStatus::Todo), // overdue, before the week
            task(2, d(0), TaskStatus::Todo),  // Monday, before today: overdue
            task(3, d(2), TaskStatus::Todo),  // today
            task(4, d(4), TaskStatus::Doing), // Friday
            task(5, d(4), TaskStatus::Done),  // done: excluded
            task(6, d(-5), TaskStatus::Done), // done and overdue: excluded
            task(7, d(9), TaskStatus::Todo),  // next week: later
            task(8, None, TaskStatus::Todo),  // no date: later
            task(9, d(6), TaskStatus::Todo),  // Sunday
            task(10, d(2), TaskStatus::Todo), // today, after 3 by position
        ]);
        let g = Groups::new(&data);

        assert_eq!(ids(&g.overdue), vec![1, 2]);
        assert_eq!(g.days.len(), 7);
        assert_eq!(g.days[0].0, date(2026, 11, 16));
        assert_eq!(g.days[6].0, date(2026, 11, 22));
        assert!(g.days[0].1.is_empty());
        assert_eq!(ids(&g.days[2].1), vec![3, 10]);
        assert_eq!(ids(&g.days[4].1), vec![4]);
        assert_eq!(ids(&g.days[6].1), vec![9]);
        let in_days: usize = g.days.iter().map(|(_, t)| t.len()).sum();
        assert_eq!(in_days, 4);
        assert_eq!(g.later, 2);
    }

    #[test]
    fn load_returns_early_when_data_is_unchanged() {
        let mut view = WeekView::new();
        let mut d = data(vec![]);
        d.advice = Some(report("**Focus**: ship it"));
        view.load(d.clone());
        assert_eq!(view.advice_src.as_deref(), Some("**Focus**: ship it"));

        // Poison the cached source: an unchanged load must not reparse.
        view.advice_src = Some("sentinel".into());
        view.show_facts = true;
        view.load(d.clone());
        assert_eq!(view.advice_src.as_deref(), Some("sentinel"));
        assert!(view.show_facts);

        // A change (same week) reparses and keeps the facts toggle.
        d.generating = true;
        d.advice = Some(report("new"));
        view.load(d.clone());
        assert_eq!(view.advice_src.as_deref(), Some("new"));
        assert!(view.show_facts);

        // A different week closes the facts.
        d.week = d.week.next();
        view.load(d);
        assert!(!view.show_facts);
    }

    #[test]
    fn update_maps_messages_to_actions() {
        let mut view = WeekView::new();
        view.load(data(vec![task(1, None, TaskStatus::Todo)]));
        assert_eq!(
            view.update(Message::Toggle(1, true)),
            Action::SetDone(1, true)
        );
        assert_eq!(
            view.update(Message::Toggle(1, false)),
            Action::SetDone(1, false)
        );
        assert_eq!(view.update(Message::Generate), Action::Generate);
        assert_eq!(view.update(Message::ToggleFacts), Action::None);
        assert!(view.show_facts);
        assert_eq!(
            view.update(Message::LinkClicked("https://example.com".into())),
            Action::None
        );
    }
}
