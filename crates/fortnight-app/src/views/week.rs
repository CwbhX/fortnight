//! The Week tab (PLAN.md 4.2 "Week"). Owner: agent K (round 2).
//!
//! One column per working day, in week order, all the same width and
//! height. Each column has a heading (weekday and date; click to open the
//! day in the Today tab), a load line once an open task that day has an
//! estimate ("3.5h of 6h", red when over capacity, "+2 unestimated"), then
//! the day's tasks: open ones first, done ones dimmed after them. A task is
//! placed by `Task::day()` (planned day, else deadline). Non-working days
//! share one extra column ("Sat–Sun"), shown only when one of them has
//! tasks. Today's column has the accent ring and, when tasks from before
//! this week are overdue, a quiet "2 overdue — see Today" link. The report
//! panel sits below the columns in a card. The whole tab scrolls.
//!
//! Skipped on purpose: drag-and-drop between days (PLAN.md 4.2; a task is
//! moved by editing its date in the Today tab), dropped tasks (not shown
//! here; the Today tab has them), editing in place (click a heading to edit
//! the day), and a private marker.

use crate::theme;
use crate::views::report_panel::{self, ReportPanel, ReportPanelData};
use fortnight_core::domain::{Task, TaskId, TaskStatus, WeekRef};
use fortnight_core::report::DayLoad;
use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, button, checkbox, column, container, progress_bar, row, scrollable, space, stack,
    text,
};
use iced::{Center, Element, Fill, Theme};
use jiff::civil::{Date, Weekday};

/// The shortest a day column gets, so an empty week still looks like a week.
const MIN_COLUMN_HEIGHT: f32 = 220.0;

#[derive(Debug, Clone)]
pub enum Message {
    /// A task's checkbox.
    Toggle(TaskId, bool),
    /// A day's heading (or the overdue note) was clicked.
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
        let specs = columns(data);
        let overdue = overdue_before_week(data);

        // Equal heights: the backgrounds are pushed under the contents in a
        // stack, so they take the height of the tallest column (iced's row
        // can't stretch its children to the tallest one inside a scrollable).
        let mut backgrounds = Row::new().spacing(theme::SPACING);
        let mut contents = Row::new().spacing(theme::SPACING);
        for spec in &specs {
            backgrounds = backgrounds.push(
                container(space())
                    .width(Fill)
                    .height(Fill)
                    .style(theme::day_column(spec.is_today)),
            );
            let note = if spec.is_today { overdue } else { 0 };
            contents = contents.push(day_column(data, spec, note));
        }
        let base = row![
            space().width(0).height(MIN_COLUMN_HEIGHT),
            contents.width(Fill)
        ];
        let grid = stack![base].push_under(backgrounds).width(Fill);

        let body = column![
            grid,
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

/// One column of the grid: the days it lists, in order.
#[derive(Debug, Clone, PartialEq)]
struct ColumnSpec {
    days: Vec<Date>,
    /// A working day's own column, or the shared non-working one.
    weekend: bool,
    is_today: bool,
}

/// The columns to show: every working day in week order, then the shared
/// non-working column when one of its days has tasks.
fn columns(data: &WeekData) -> Vec<ColumnSpec> {
    let days = data.week.days();
    let (working, other): (Vec<Date>, Vec<Date>) = days
        .into_iter()
        .partition(|d| data.working_days.contains(&d.weekday()));
    let mut out: Vec<ColumnSpec> = working
        .into_iter()
        .map(|d| ColumnSpec {
            days: vec![d],
            weekend: false,
            is_today: d == data.today,
        })
        .collect();
    if other.iter().any(|d| !tasks_on(&data.tasks, *d).is_empty()) {
        out.push(ColumnSpec {
            is_today: other.contains(&data.today),
            days: other,
            weekend: true,
        });
    }
    out
}

/// Tasks whose `Task::day()` is `day`, without dropped ones: open first,
/// then done; by position.
pub fn tasks_on(tasks: &[Task], day: Date) -> Vec<&Task> {
    let mut out: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.day() == Some(day) && t.status != TaskStatus::Dropped)
        .collect();
    out.sort_by(|a, b| {
        (!a.status.is_open())
            .cmp(&!b.status.is_open())
            .then(a.position.total_cmp(&b.position))
            .then(a.id.cmp(&b.id))
    });
    out
}

/// Open tasks overdue today that sit on a day before the week (they don't
/// appear in any column). Zero unless the week contains today.
fn overdue_before_week(data: &WeekData) -> usize {
    if !data.week.contains(data.today) {
        return 0;
    }
    data.tasks
        .iter()
        .filter(|t| t.is_overdue(data.today) && t.day().is_some_and(|d| d < data.week.start))
        .count()
}

/// The load line under a column heading.
#[derive(Debug, Clone, PartialEq)]
struct LoadLine {
    /// Load over capacity, 0–1, for the bar; `None` without capacity (a
    /// non-working day), where only the hours are shown.
    fraction: Option<f32>,
    over: bool,
    /// "3.5h of 6h", or "2h" without capacity.
    label: String,
    /// "+2 unestimated", when some open tasks have no estimate.
    unestimated: Option<String>,
}

/// The load line for these days' loads added together; `None` until an
/// open task on them has an estimate.
fn load_line(loads: &[&DayLoad]) -> Option<LoadLine> {
    let load: u32 = loads.iter().map(|l| l.load_minutes).sum();
    let capacity: u32 = loads.iter().map(|l| l.capacity_minutes).sum();
    let unestimated: usize = loads.iter().map(|l| l.unestimated).sum();
    if load == 0 {
        return None;
    }
    let (fraction, label) = if capacity == 0 {
        (None, duration(load))
    } else {
        (
            Some((load as f32 / capacity as f32).min(1.0)),
            format!("{} of {}", duration(load), duration(capacity)),
        )
    };
    Some(LoadLine {
        fraction,
        over: capacity > 0 && load > capacity,
        label,
        unestimated: (unestimated > 0).then(|| format!("+{unestimated} unestimated")),
    })
}

/// Minutes as a short duration: 30 → "30m", 90 → "1.5h", 120 → "2h".
fn duration(minutes: u32) -> String {
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let tenths = (minutes * 10 + 30) / 60;
    if tenths.is_multiple_of(10) {
        format!("{}h", tenths / 10)
    } else {
        format!("{}.{}h", tenths / 10, tenths % 10)
    }
}

/// The column heading: the weekday (or "Sat–Sun") and the date(s).
fn heading_text(spec: &ColumnSpec) -> (String, String) {
    let first = spec.days[0];
    let last = spec.days[spec.days.len() - 1];
    if spec.days.len() == 1 {
        return (first.strftime("%a").to_string(), date_label(first));
    }
    let consecutive = spec
        .days
        .windows(2)
        .all(|w| w[0].tomorrow().ok() == Some(w[1]));
    if !consecutive {
        let names: Vec<String> = spec
            .days
            .iter()
            .map(|d| d.strftime("%a").to_string())
            .collect();
        return (names.join(", "), String::new());
    }
    let names = format!("{} to {}", first.strftime("%a"), last.strftime("%a"));
    let dates = if first.month() == last.month() {
        format!("{} to {}", first.day(), date_label(last))
    } else {
        format!("{} to {}", date_label(first), date_label(last))
    };
    (names, dates)
}

/// "16 Nov".
fn date_label(d: Date) -> String {
    d.strftime("%-d %b").to_string()
}

/// The small grey line under a task title: estimate, project, the deadline
/// when the task is planned for another day, and "in progress".
fn meta_line(task: &Task, column_day: Date) -> String {
    let mut parts = Vec::new();
    if let Some(m) = task.estimate_minutes {
        parts.push(duration(m));
    }
    if let Some(p) = &task.project {
        parts.push(format!("#{p}"));
    }
    if let Some(due) = task.due_on
        && due != column_day
    {
        parts.push(format!("due {}", due.strftime("%a %-d")));
    }
    if task.status == TaskStatus::Doing {
        parts.push("in progress".into());
    }
    parts.join(" · ")
}

fn day_column<'a>(data: &'a WeekData, spec: &ColumnSpec, overdue: usize) -> Element<'a, Message> {
    let (name, dates) = heading_text(spec);
    let name_style: fn(&Theme) -> text::Style = if spec.is_today {
        theme::accent
    } else {
        text::default
    };
    let open_day = spec
        .days
        .iter()
        .copied()
        .find(|d| !tasks_on(&data.tasks, *d).is_empty())
        .unwrap_or(spec.days[0]);
    let heading = button(
        row![
            text(name).size(theme::TEXT_BODY).style(name_style),
            text(dates).size(theme::TEXT_SMALL).style(theme::muted),
        ]
        .spacing(6)
        .align_y(Center),
    )
    .padding(0)
    .width(Fill)
    .style(theme::text_button)
    .on_press(Message::OpenDay(open_day));

    let mut col = Column::new().spacing(8).width(Fill).push(heading);

    let loads: Vec<&DayLoad> = data
        .loads
        .iter()
        .filter(|l| spec.days.contains(&l.date))
        .collect();
    if let Some(line) = load_line(&loads) {
        col = col.push(load_view(line));
    }

    if overdue > 0 {
        col = col.push(
            button(
                text(format!("{overdue} overdue, see Today"))
                    .size(theme::TEXT_SMALL)
                    .style(theme::danger),
            )
            .padding(0)
            .style(theme::text_button)
            .on_press(Message::OpenDay(data.today)),
        );
    }

    let mut list = Column::new().spacing(8).width(Fill);
    let mut any = false;
    for day in &spec.days {
        let tasks = tasks_on(&data.tasks, *day);
        if tasks.is_empty() {
            continue;
        }
        if spec.weekend && spec.days.len() > 1 {
            list = list.push(theme::section_heading(day.strftime("%a %-d").to_string()));
        }
        for task in tasks {
            any = true;
            list = list.push(task_row(task, *day, data.today));
        }
    }
    if !any {
        list = list.push(
            text("Nothing planned")
                .size(theme::TEXT_SMALL)
                .style(theme::muted),
        );
    }
    col = col.push(container(list).padding([4, 0]));

    container(col).padding(theme::SPACING).width(Fill).into()
}

fn load_view<'a>(line: LoadLine) -> Element<'a, Message> {
    let mut col = Column::new().spacing(4);
    if let Some(f) = line.fraction {
        col = col.push(
            progress_bar(0.0..=1.0, f)
                .girth(4)
                .style(theme::load_bar(line.over)),
        );
    }
    let label_style: fn(&Theme) -> text::Style = if line.over {
        theme::danger
    } else {
        theme::muted
    };
    let mut labels = row![
        text(line.label)
            .size(theme::TEXT_SMALL)
            .style(label_style)
            .wrapping(Wrapping::None)
    ]
    .spacing(6);
    if let Some(u) = line.unestimated {
        labels = labels.push(
            text(u)
                .size(theme::TEXT_SMALL)
                .style(theme::muted)
                .wrapping(Wrapping::None),
        );
    }
    // In a narrow column "+2 unestimated" moves to its own line whole.
    col.push(labels.wrap().vertical_spacing(2)).into()
}

fn task_row<'a>(task: &'a Task, day: Date, today: Date) -> Element<'a, Message> {
    let id = task.id;
    let done = !task.status.is_open();
    let overdue = task.is_overdue(today);
    let title_style: fn(&Theme) -> text::Style = if done {
        theme::muted
    } else if overdue {
        theme::danger
    } else {
        text::default
    };

    let mut title = Row::new().spacing(4);
    if !done && task.priority > 0 {
        let p = task.priority;
        title = title.push(
            text("!".repeat(p.min(3) as usize))
                .size(theme::TEXT_SMALL)
                .style(move |t: &Theme| text::Style {
                    color: theme::priority_color(t, p),
                }),
        );
    }
    title = title.push(
        text(task.title.as_str())
            .size(theme::TEXT_SMALL)
            .style(title_style)
            .wrapping(Wrapping::WordOrGlyph)
            .width(Fill),
    );

    let mut body = Column::new().spacing(2).width(Fill).push(title);
    if !done {
        let meta = meta_line(task, day);
        if !meta.is_empty() {
            body = body.push(
                text(meta)
                    .size(11)
                    .style(theme::muted)
                    .wrapping(Wrapping::WordOrGlyph),
            );
        }
    }

    row![
        // Nudged down to sit on the title's first line.
        container(
            checkbox(task.is_done())
                .size(14)
                .on_toggle(move |b| Message::Toggle(id, b))
        )
        .padding(iced::Padding::ZERO.top(2)),
        body,
    ]
    .spacing(8)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fortnight_core::domain::ReportKind;
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

    fn mon_fri() -> Vec<Weekday> {
        vec![
            Weekday::Monday,
            Weekday::Tuesday,
            Weekday::Wednesday,
            Weekday::Thursday,
            Weekday::Friday,
        ]
    }

    fn data(tasks: Vec<Task>) -> WeekData {
        let week = WeekRef::new(date(2026, 11, 16));
        WeekData {
            week,
            today: date(2026, 11, 18),
            tasks,
            working_days: mon_fri(),
            loads: Vec::new(),
            panel: ReportPanelData {
                week,
                week_label: "Week 47".into(),
                slots: Vec::new(),
            },
        }
    }

    fn load(date: Date, load: u32, capacity: u32, unestimated: usize) -> DayLoad {
        DayLoad {
            date,
            load_minutes: load,
            capacity_minutes: capacity,
            unestimated,
        }
    }

    #[test]
    fn tasks_on_uses_planned_day_and_puts_open_first() {
        let d = date(2026, 11, 18);
        let mut planned = task(3, Some(date(2026, 11, 25)), TaskStatus::Todo);
        planned.planned_for = Some(d);
        let mut moved_away = task(5, Some(d), TaskStatus::Todo);
        moved_away.planned_for = Some(date(2026, 11, 19));
        let tasks = vec![
            task(1, Some(d), TaskStatus::Done),
            task(2, Some(d), TaskStatus::Todo),
            planned,
            task(4, Some(date(2026, 11, 19)), TaskStatus::Todo),
            moved_away,
            task(6, Some(d), TaskStatus::Dropped),
            task(7, Some(d), TaskStatus::Doing),
        ];
        let ids: Vec<_> = tasks_on(&tasks, d).iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![2, 3, 7, 1]);
        let ids: Vec<_> = tasks_on(&tasks, date(2026, 11, 19))
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![4, 5]);
    }

    #[test]
    fn columns_are_the_working_days_and_mark_today() {
        let cols = columns(&data(vec![]));
        assert_eq!(cols.len(), 5);
        assert!(cols.iter().all(|c| !c.weekend && c.days.len() == 1));
        assert_eq!(cols[0].days, vec![date(2026, 11, 16)]);
        assert_eq!(cols[4].days, vec![date(2026, 11, 20)]);
        let today: Vec<_> = cols.iter().map(|c| c.is_today).collect();
        assert_eq!(today, vec![false, false, true, false, false]);
    }

    #[test]
    fn weekend_column_only_when_a_weekend_day_has_tasks() {
        // A dropped weekend task, and one from another week: no column.
        let cols = columns(&data(vec![
            task(1, Some(date(2026, 11, 22)), TaskStatus::Dropped),
            task(2, Some(date(2026, 11, 29)), TaskStatus::Todo),
        ]));
        assert_eq!(cols.len(), 5);

        let cols = columns(&data(vec![task(
            1,
            Some(date(2026, 11, 22)),
            TaskStatus::Done,
        )]));
        assert_eq!(cols.len(), 6);
        let weekend = &cols[5];
        assert!(weekend.weekend);
        assert_eq!(weekend.days, vec![date(2026, 11, 21), date(2026, 11, 22)]);
        assert_eq!(
            heading_text(weekend),
            ("Sat to Sun".to_string(), "21 to 22 Nov".to_string())
        );
    }

    #[test]
    fn headings_show_weekday_and_date() {
        let cols = columns(&data(vec![]));
        assert_eq!(
            heading_text(&cols[0]),
            ("Mon".to_string(), "16 Nov".to_string())
        );
    }

    #[test]
    fn load_line_from_day_loads() {
        let d = date(2026, 11, 18);
        assert_eq!(load_line(&[&load(d, 0, 360, 2)]), None);

        let line = load_line(&[&load(d, 210, 360, 0)]).unwrap();
        assert_eq!(line.label, "3.5h of 6h");
        assert!(!line.over);
        assert_eq!(line.fraction, Some(210.0 / 360.0));
        assert_eq!(line.unestimated, None);

        let line = load_line(&[&load(d, 450, 360, 2)]).unwrap();
        assert_eq!(line.label, "7.5h of 6h");
        assert!(line.over);
        assert_eq!(line.fraction, Some(1.0));
        assert_eq!(line.unestimated.as_deref(), Some("+2 unestimated"));

        // Exactly at capacity is not over.
        assert!(!load_line(&[&load(d, 360, 360, 0)]).unwrap().over);

        // Non-working days: no capacity, no bar, never "over".
        let sat = load(date(2026, 11, 21), 60, 0, 0);
        let sun = load(date(2026, 11, 22), 30, 0, 1);
        let line = load_line(&[&sat, &sun]).unwrap();
        assert_eq!(line.label, "1.5h");
        assert_eq!(line.fraction, None);
        assert!(!line.over);
        assert_eq!(line.unestimated.as_deref(), Some("+1 unestimated"));
    }

    #[test]
    fn durations_are_short() {
        assert_eq!(duration(30), "30m");
        assert_eq!(duration(60), "1h");
        assert_eq!(duration(90), "1.5h");
        assert_eq!(duration(100), "1.7h");
        assert_eq!(duration(480), "8h");
    }

    #[test]
    fn meta_line_shows_estimate_project_and_other_deadline() {
        let d = date(2026, 11, 18);
        let mut t = task(1, Some(date(2026, 11, 20)), TaskStatus::Doing);
        t.planned_for = Some(d);
        t.estimate_minutes = Some(120);
        t.project = Some("work".into());
        assert_eq!(meta_line(&t, d), "2h · #work · due Fri 20 · in progress");
        let plain = task(2, Some(d), TaskStatus::Todo);
        assert_eq!(meta_line(&plain, d), "");
    }

    #[test]
    fn overdue_note_counts_only_tasks_before_the_week() {
        let mut planned_here = task(3, Some(date(2026, 11, 2)), TaskStatus::Todo);
        planned_here.planned_for = Some(date(2026, 11, 17));
        let d = data(vec![
            task(1, Some(date(2026, 11, 10)), TaskStatus::Todo),
            task(2, Some(date(2026, 11, 12)), TaskStatus::Done),
            planned_here,
            // Overdue but in this week's Monday column already.
            task(4, Some(date(2026, 11, 16)), TaskStatus::Todo),
        ]);
        assert_eq!(overdue_before_week(&d), 1);

        let mut past = d.clone();
        past.week = WeekRef::new(date(2026, 11, 23));
        assert_eq!(overdue_before_week(&past), 0);
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
            v.update(Message::Toggle(4, false)),
            Action::SetStatus(4, TaskStatus::Todo)
        );
        assert_eq!(
            v.update(Message::OpenDay(date(2026, 11, 17))),
            Action::OpenDay(date(2026, 11, 17))
        );
    }

    #[test]
    fn load_keeps_or_replaces_data() {
        let mut v = WeekView::new();
        let first = data(vec![task(1, Some(date(2026, 11, 17)), TaskStatus::Todo)]);
        v.load(first.clone());
        v.load(first.clone());
        assert_eq!(v.data.as_ref(), Some(&first));
        let second = data(vec![]);
        v.load(second.clone());
        assert_eq!(v.data.as_ref(), Some(&second));
    }

    #[test]
    fn panel_messages_are_forwarded() {
        let mut v = WeekView::new();
        let mut d = data(vec![]);
        d.panel.slots.push(report_panel::ReportSlot {
            kind: ReportKind::Plan,
            facts_md: "facts".into(),
            preview: String::new(),
            versions: Vec::new(),
            streaming: None,
            blocked: None,
        });
        let week = d.week;
        v.load(d);
        assert_eq!(
            v.update(Message::Panel(report_panel::Message::Generate)),
            Action::Panel(report_panel::Action::Generate(ReportKind::Plan, week))
        );
    }
}
