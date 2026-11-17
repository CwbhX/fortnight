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
//! this week are overdue, a quiet "2 overdue, see Today" link. The whole tab
//! scrolls.
//!
//! The AI parts come from the report panel and are generated automatically
//! (see `report_panel`): the week's focus sentence above the columns, a
//! numbered badge on each recommended task in its column, then a
//! Recommended tasks card (the order to work in, with a few words on why)
//! and the Work summary card.
//!
//! Tasks are small cards. An open task's card can be dragged: a copy follows
//! the pointer, the day under it is highlighted, and an accent line shows
//! where it will land. Dropping on a card puts the task before it; dropping
//! on a column's free space puts it after that day's open tasks. Another day
//! becomes the task's due date (clearing an old planned day); within a day
//! only the order (`Task::position`) changes. Dropping on the weekend
//! column's free space uses its first day; letting go anywhere else cancels.
//!
//! Skipped on purpose: deleted tasks (the Today tab has them) and editing in
//! place (click a heading to edit the day).

use crate::theme;
use crate::views::report_panel::{self, ReportPanel, ReportPanelData};
use fortnight_core::domain::{ReportKind, Task, TaskId, TaskStatus, WeekRef};
use fortnight_core::report::DayLoad;
use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, button, checkbox, column, container, mouse_area, pin, progress_bar, row,
    scrollable, space, stack, text,
};
use iced::{Center, Color, Element, Fill, Point, Theme, mouse};
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
    /// The mouse went down on a task card.
    Press(TaskId),
    /// The pointer moved while a card is held (position in the tab).
    Moved(Point),
    /// While dragging, the pointer entered a day column's free space
    /// (`None`) or the card of this task.
    Hover(Date, Option<TaskId>),
    /// A dragged card was let go there.
    DropOn(Date, Option<TaskId>),
    /// The mouse button went up: the drag (if any) is over.
    EndDrag,
}

impl Message {
    /// Pointer bookkeeping that changes nothing but the drag: `main.rs`
    /// skips its refresh for these, as they arrive with every mouse move.
    pub fn is_pointer_move(&self) -> bool {
        matches!(
            self,
            Message::Press(_) | Message::Moved(_) | Message::Hover(..) | Message::EndDrag
        )
    }
}

/// How far the pointer must move before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 4.0;

/// A held task card.
#[derive(Debug, Clone, PartialEq)]
struct Drag {
    id: TaskId,
    /// Where the pointer was first seen after the press.
    start: Option<Point>,
    /// Where it is now.
    at: Option<Point>,
    /// Moved past `DRAG_THRESHOLD`: a drag, not a click.
    active: bool,
    /// The day under the pointer, and the card it is over (`None`: the
    /// column's free space).
    over: Option<(Date, Option<TaskId>)>,
}

/// What `main.rs` must do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    SetStatus(TaskId, TaskStatus),
    /// Show this day in the Today tab.
    OpenDay(Date),
    /// Something the embedded report panel asked for.
    Panel(report_panel::Action),
    /// A card was dragged: make `Date` its due date (if it isn't already
    /// its day) and set its position to the `f64`.
    Move(TaskId, Date, f64),
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
    drag: Option<Drag>,
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
            Message::Press(id) => {
                self.drag = Some(Drag {
                    id,
                    start: None,
                    at: None,
                    active: false,
                    over: None,
                });
                Action::None
            }
            Message::Moved(p) => {
                if let Some(drag) = &mut self.drag {
                    let start = *drag.start.get_or_insert(p);
                    drag.at = Some(p);
                    if !drag.active && start.distance(p) > DRAG_THRESHOLD {
                        drag.active = true;
                    }
                }
                Action::None
            }
            Message::Hover(day, card) => {
                if let Some(drag) = &mut self.drag {
                    drag.over = Some((day, card));
                }
                Action::None
            }
            Message::DropOn(day, before) => {
                let Some(drag) = self.drag.take().filter(|d| d.active) else {
                    return Action::None;
                };
                let Some(data) = &self.data else {
                    return Action::None;
                };
                match drop_position(&data.tasks, drag.id, day, before) {
                    Some(position) => Action::Move(drag.id, day, position),
                    None => Action::None,
                }
            }
            Message::EndDrag => {
                self.drag = None;
                Action::None
            }
        }
    }

    /// The task being dragged (past the threshold), if any.
    fn dragging(&self) -> Option<&Drag> {
        self.drag.as_ref().filter(|d| d.active)
    }

    pub fn view(&self) -> Element<'_, Message> {
        let Some(data) = &self.data else {
            return text("Loading…").style(theme::muted).into();
        };
        let specs = columns(data);
        let overdue = overdue_before_week(data);
        let ranks = self.ranks();
        let dragging = self.dragging();

        // Equal heights: the backgrounds are pushed under the contents in a
        // stack, so they take the height of the tallest column (iced's row
        // can't stretch its children to the tallest one inside a scrollable).
        let mut backgrounds = Row::new().spacing(theme::SPACING);
        let mut contents = Row::new().spacing(theme::SPACING);
        for spec in &specs {
            let day = spec.days[0];
            let mut free =
                mouse_area(container(space()).width(Fill).height(Fill).style(
                    drop_target(
                        spec.is_today,
                        dragging.is_some_and(|d| {
                            d.over.is_some_and(|(day, _)| spec.days.contains(&day))
                        }),
                    ),
                ));
            if dragging.is_some() {
                // The column's free space: drop after its open tasks.
                free = free
                    .on_enter(Message::Hover(day, None))
                    .on_release(Message::DropOn(day, None));
            }
            backgrounds = backgrounds.push(free);
            let note = if spec.is_today { overdue } else { 0 };
            contents = contents.push(day_column(data, spec, note, &ranks, dragging));
        }
        let base = row![
            space().width(0).height(MIN_COLUMN_HEIGHT),
            contents.width(Fill)
        ];
        let grid = stack![base].push_under(backgrounds).width(Fill);

        let mut body = Column::new().spacing(theme::SPACING * 2.0);
        if let Some(focus) = self
            .panel
            .recommendation()
            .map(|r| r.focus.as_str())
            .filter(|f| !f.is_empty())
        {
            body = body.push(text(focus).size(theme::TEXT_HEADING));
        }
        body = body.push(grid);
        if self.panel.slot(ReportKind::Plan).is_some() {
            body = body.push(card(
                "Recommended tasks",
                self.panel
                    .status_view(ReportKind::Plan)
                    .map(|e| e.map(Message::Panel)),
                self.recommended(data),
            ));
        }
        if self.panel.slot(ReportKind::Summary).is_some() {
            body = body.push(card(
                "Work summary",
                self.panel
                    .status_view(ReportKind::Summary)
                    .map(|e| e.map(Message::Panel)),
                self.panel.summary_view().map(Message::Panel),
            ));
        }
        let mut layers = stack![body.padding(theme::SPACING * 2.0)];
        if let Some(drag) = dragging
            && let Some(at) = drag.at
            && let Some(task) = data.tasks.iter().find(|t| t.id == drag.id)
        {
            let ghost = container(task_card(
                task,
                task.day().unwrap_or(data.today),
                data.today,
                None,
                false,
            ))
            .width(200)
            .style(|_: &Theme| container::Style {
                shadow: iced::Shadow {
                    color: Color::BLACK.scale_alpha(0.35),
                    offset: iced::Vector::new(0.0, 6.0),
                    blur_radius: 16.0,
                },
                ..container::Style::default()
            });
            layers = layers.push(pin(ghost).position(Point::new(at.x - 24.0, at.y - 16.0)));
        }
        let mut area = mouse_area(layers);
        if self.drag.is_some() {
            area = area.on_move(Message::Moved).on_release(Message::EndDrag);
        }
        scrollable(area).height(Fill).into()
    }

    /// Each recommended task that still exists and isn't deleted, with its
    /// rank (1 = do first).
    fn ranked<'a>(&'a self, data: &'a WeekData) -> Vec<(usize, &'a Task, Option<Date>, &'a str)> {
        let Some(rec) = self.panel.recommendation() else {
            return Vec::new();
        };
        rec.tasks
            .iter()
            .filter_map(|r| {
                let task = data.tasks.iter().find(|t| t.id == r.id)?;
                (task.status != TaskStatus::Dropped).then_some((task, r.day, r.why.as_str()))
            })
            .enumerate()
            .map(|(i, (t, day, why))| (i + 1, t, day, why))
            .collect()
    }

    /// Task id to rank, for the badges in the day columns.
    fn ranks(&self) -> Vec<(TaskId, usize)> {
        let Some(data) = &self.data else {
            return Vec::new();
        };
        self.ranked(data)
            .into_iter()
            .map(|(rank, t, _, _)| (t.id, rank))
            .collect()
    }

    /// The Recommended tasks list: rank, checkbox, title, suggested day and
    /// why. Text the model returned in another shape is shown as it is.
    fn recommended<'a>(&'a self, data: &'a WeekData) -> Element<'a, Message> {
        if let Some(raw) = self.panel.unparsed_plan() {
            return text(raw).size(theme::TEXT_BODY).into();
        }
        let ranked = self.ranked(data);
        if ranked.is_empty() {
            return Column::new().into();
        }
        Column::with_children(ranked.into_iter().map(|(rank, task, day, why)| {
            let id = task.id;
            let done = !task.status.is_open();
            let mut line = row![
                rank_badge(rank, done),
                checkbox(task.is_done()).on_toggle(move |b| Message::Toggle(id, b)),
                text(task.title.as_str())
                    .size(theme::TEXT_BODY)
                    .style(if done { theme::muted } else { text::default })
                    .wrapping(Wrapping::WordOrGlyph),
            ]
            .spacing(theme::SPACING * 0.75)
            .align_y(Center);
            if let Some(day) = day {
                line = line.push(
                    container(text(day.strftime("%a").to_string()).size(theme::TEXT_SMALL))
                        .padding(theme::chip_padding())
                        .style(theme::chip),
                );
            }
            if !why.is_empty() {
                line = line.push(
                    text(why)
                        .size(theme::TEXT_SMALL)
                        .style(theme::muted)
                        .wrapping(Wrapping::WordOrGlyph),
                );
            }
            line.into()
        }))
        .spacing(theme::SPACING * 0.75)
        .into()
    }
}

/// A titled card: the heading with its status line, then the body.
fn card<'a>(
    heading: &'a str,
    status: Option<Element<'a, Message>>,
    body: Element<'a, Message>,
) -> Element<'a, Message> {
    let mut head = row![text(heading).size(theme::TEXT_HEADING)]
        .spacing(theme::SPACING)
        .align_y(Center);
    if let Some(status) = status {
        head = head.push(status);
    }
    container(column![head, body].spacing(theme::SPACING * 1.25))
        .padding(theme::SPACING * 1.5)
        .width(Fill)
        .style(theme::card)
        .into()
}

/// A small round number in the accent colour: a recommended task's rank.
fn rank_badge<'a>(rank: usize, done: bool) -> Element<'a, Message> {
    container(
        text(rank.to_string())
            .size(11)
            .color(iced::Color::WHITE)
            .align_x(Center),
    )
    .center_x(18)
    .center_y(18)
    .style(move |_: &Theme| container::Style {
        background: Some(
            if done {
                theme::ACCENT.scale_alpha(0.4)
            } else {
                theme::ACCENT
            }
            .into(),
        ),
        border: iced::border::rounded(999),
        ..container::Style::default()
    })
    .into()
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

/// The small grey line under a task title: estimate, project, and the
/// deadline when the task is planned for another day.
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
    parts.join(" · ")
}

fn day_column<'a>(
    data: &'a WeekData,
    spec: &ColumnSpec,
    overdue: usize,
    ranks: &[(TaskId, usize)],
    drag: Option<&Drag>,
) -> Element<'a, Message> {
    let dragged = drag.map(|d| d.id);
    let over = drag.and_then(|d| d.over);
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

    // Cards sit on fixed 8 px slots that double as the landing line, so
    // showing the line never moves anything (which would move the target
    // under the pointer and make the line jump back and forth).
    let mut list = Column::new().width(Fill);
    let mut any = false;
    // The line shows only where a drop would change something, and only
    // among other open tasks (not in an empty day).
    let shows_line = |day: Date, before: Option<TaskId>| {
        dragged.is_some_and(|id| {
            over == Some((day, before)) && line_wanted(&data.tasks, id, day, before)
        })
    };
    for day in &spec.days {
        let tasks = tasks_on(&data.tasks, *day);
        if tasks.is_empty() {
            continue;
        }
        if spec.weekend && spec.days.len() > 1 {
            list = list.push(theme::section_heading(day.strftime("%a %-d").to_string()));
        }
        let open = tasks.iter().filter(|t| t.status.is_open()).count();
        for (i, task) in tasks.into_iter().enumerate() {
            any = true;
            let rank = ranks.iter().find(|(id, _)| *id == task.id).map(|r| r.1);
            list = list.push(task_row(
                task,
                *day,
                data.today,
                rank,
                dragged.is_some().then_some(dragged == Some(task.id)),
                shows_line(*day, Some(task.id)),
            ));
            if i + 1 == open {
                list = list.push(slot(shows_line(*day, None)));
            }
        }
    }
    if !any {
        list = list.push(
            text("Nothing planned")
                .size(theme::TEXT_SMALL)
                .style(theme::muted),
        );
    }
    col = col.push(list);

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

/// A task card in a day column: draggable when open. `dragging` is `None`
/// when nothing is being dragged, else whether this is the dragged card
/// (shown faded). While dragging, an open card is a drop target: the
/// dragged task goes before it.
fn task_row<'a>(
    task: &'a Task,
    day: Date,
    today: Date,
    rank: Option<usize>,
    dragging: Option<bool>,
    line_above: bool,
) -> Element<'a, Message> {
    let card = column![
        slot(line_above),
        task_card(task, day, today, rank, dragging == Some(true))
    ];
    if !task.status.is_open() {
        return card.into();
    }
    // The slot above belongs to the card: hovering the gap means "before
    // this card" too.
    let mut area = mouse_area(card)
        .on_press(Message::Press(task.id))
        .interaction(if dragging.is_some() {
            mouse::Interaction::Grabbing
        } else {
            mouse::Interaction::Grab
        });
    if dragging.is_some() {
        area = area
            .on_enter(Message::Hover(day, Some(task.id)))
            .on_release(Message::DropOn(day, Some(task.id)));
    }
    area.into()
}

/// Whether dropping task `id` on `day` before `before` deserves the landing
/// line: another open task is there to order against, and the drop would
/// change something.
fn line_wanted(tasks: &[Task], id: TaskId, day: Date, before: Option<TaskId>) -> bool {
    tasks_on(tasks, day)
        .iter()
        .any(|t| t.status.is_open() && t.id != id)
        && drop_position(tasks, id, day, before).is_some()
}

/// The 8 px gap above a card (and after a day's open cards): empty, or the
/// accent line that shows where a dragged card will land.
fn slot<'a>(line: bool) -> Element<'a, Message> {
    let inner: Element<'a, Message> = if line {
        container(space())
            .width(Fill)
            .height(3)
            .style(|_: &Theme| container::Style {
                background: Some(theme::ACCENT.into()),
                border: iced::border::rounded(2),
                ..container::Style::default()
            })
            .into()
    } else {
        space().into()
    };
    container(inner).width(Fill).height(8).center_y(8).into()
}

/// Where task `id` goes when dropped on `day`, before the open task
/// `before` (or after the day's open tasks): its new `position`, between its
/// new neighbours. `None` when nothing would change (same day, same place).
pub fn drop_position(tasks: &[Task], id: TaskId, day: Date, before: Option<TaskId>) -> Option<f64> {
    let task = tasks.iter().find(|t| t.id == id)?;
    let open: Vec<&Task> = tasks_on(tasks, day)
        .into_iter()
        .filter(|t| t.status.is_open())
        .collect();
    let others: Vec<&Task> = open.iter().copied().filter(|t| t.id != id).collect();
    let index = match before {
        Some(b) if b != id => others
            .iter()
            .position(|t| t.id == b)
            .unwrap_or(others.len()),
        Some(_) => return None,
        None => others.len(),
    };
    if task.day() == Some(day) {
        let now = open.iter().position(|t| t.id == id);
        if now == Some(index) {
            return None;
        }
    }
    let position = match (index.checked_sub(1).map(|i| others[i]), others.get(index)) {
        (Some(prev), Some(next)) => (prev.position + next.position) / 2.0,
        (None, Some(next)) => next.position - 1.0,
        (Some(prev), None) => prev.position + 1.0,
        (None, None) => task.position,
    };
    Some(position)
}

/// The card itself: checkbox, rank badge, priority, title and the meta line.
fn task_card<'a>(
    task: &'a Task,
    day: Date,
    today: Date,
    rank: Option<usize>,
    faded: bool,
) -> Element<'a, Message> {
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
    if let Some(rank) = rank {
        title = title.push(rank_badge(rank, done));
    }
    if !done && task.priority > 0 {
        let p = task.priority.min(2);
        title = title.push(text("!".repeat(p as usize)).size(theme::TEXT_SMALL).style(
            move |t: &Theme| text::Style {
                color: theme::priority_color(t, p),
            },
        ));
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

    container(
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
        .spacing(8),
    )
    .padding([8, 10])
    .width(Fill)
    .style(move |theme: &Theme| card_style(theme, done, faded))
    .into()
}

/// A raised, rounded card; quieter when done, see-through while dragged.
fn card_style(theme: &Theme, done: bool, faded: bool) -> container::Style {
    let p = theme.palette();
    let amount = if done { 0.05 } else { 0.1 };
    let mut fill = Color::from_rgb(
        p.background.r + (p.text.r - p.background.r) * amount,
        p.background.g + (p.text.g - p.background.g) * amount,
        p.background.b + (p.text.b - p.background.b) * amount,
    );
    if faded {
        fill = fill.scale_alpha(0.35);
    }
    container::Style {
        background: Some(fill.into()),
        text_color: faded.then(|| p.text.scale_alpha(0.4)),
        border: iced::border::rounded(6),
        ..container::Style::default()
    }
}

/// A day column's background: `theme::day_column`, with an accent ring and
/// tint when a dragged card is over it.
fn drop_target(is_today: bool, over: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let base = theme::day_column(is_today)(theme);
        if !over {
            return base;
        }
        container::Style {
            background: Some(theme::ACCENT.scale_alpha(0.08).into()),
            border: iced::Border {
                color: theme::ACCENT,
                width: 2.0,
                ..base.border
            },
            ..base
        }
    }
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
            planned_for: day,
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
        assert_eq!(meta_line(&t, d), "2h · #work · due Fri 20");
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
    fn dragging_a_card_to_another_day_moves_it() {
        let wed = date(2026, 11, 19);
        let mut v = WeekView::new();
        v.load(data(vec![task(
            1,
            Some(date(2026, 11, 17)),
            TaskStatus::Todo,
        )]));
        // A press and release without moving is a click: nothing.
        v.update(Message::Press(1));
        v.update(Message::Moved(Point::new(10.0, 10.0)));
        assert_eq!(v.update(Message::DropOn(wed, None)), Action::None);

        v.update(Message::Press(1));
        v.update(Message::Moved(Point::new(10.0, 10.0)));
        v.update(Message::Moved(Point::new(80.0, 12.0)));
        assert!(v.dragging().is_some());
        v.update(Message::Hover(wed, None));
        let _ = v.view();
        assert_eq!(
            v.update(Message::DropOn(wed, None)),
            Action::Move(1, wed, 1.0)
        );
        assert!(v.drag.is_none());

        // Let go outside a column: cancelled.
        v.update(Message::Press(1));
        v.update(Message::Moved(Point::new(0.0, 0.0)));
        v.update(Message::Moved(Point::new(50.0, 0.0)));
        v.update(Message::EndDrag);
        assert!(v.drag.is_none());
        assert!(Message::Moved(Point::ORIGIN).is_pointer_move());
        assert!(!Message::DropOn(wed, None).is_pointer_move());
    }

    #[test]
    fn drop_positions_within_and_across_days() {
        let tue = date(2026, 11, 17);
        let wed = date(2026, 11, 18);
        let mut tasks = vec![
            task(1, Some(tue), TaskStatus::Todo),
            task(2, Some(tue), TaskStatus::Todo),
            task(3, Some(tue), TaskStatus::Todo),
            task(4, Some(tue), TaskStatus::Done),
            task(5, Some(wed), TaskStatus::Todo),
        ];
        // Positions are the ids: 1, 2, 3 (open), 4 (done), 5.
        // Before the first: below it.
        assert_eq!(drop_position(&tasks, 3, tue, Some(1)), Some(0.0));
        // Between two.
        assert_eq!(drop_position(&tasks, 3, tue, Some(2)), Some(1.5));
        // To the end of the open ones.
        assert_eq!(drop_position(&tasks, 1, tue, None), Some(4.0));
        // Already there: nothing.
        assert_eq!(drop_position(&tasks, 3, tue, None), None);
        assert_eq!(drop_position(&tasks, 2, tue, Some(3)), None);
        assert_eq!(drop_position(&tasks, 2, tue, Some(2)), None);
        // Another day: before its card, or alone in an empty day.
        assert_eq!(drop_position(&tasks, 1, wed, Some(5)), Some(4.0));
        assert_eq!(drop_position(&tasks, 1, wed, None), Some(6.0));
        assert_eq!(
            drop_position(&tasks, 1, date(2026, 11, 19), None),
            Some(1.0)
        );
        tasks.clear();
        assert_eq!(drop_position(&tasks, 1, tue, None), None);

        // The landing line: not in an empty day, not for a day's only
        // task, not where nothing would change.
        let tasks = vec![
            task(1, Some(tue), TaskStatus::Todo),
            task(2, Some(tue), TaskStatus::Todo),
            task(3, Some(wed), TaskStatus::Todo),
        ];
        assert!(!line_wanted(&tasks, 1, date(2026, 11, 19), None));
        assert!(!line_wanted(&tasks, 3, wed, None));
        assert!(!line_wanted(&tasks, 2, tue, None));
        assert!(line_wanted(&tasks, 1, tue, None));
        assert!(line_wanted(&tasks, 2, tue, Some(1)));
        assert!(line_wanted(&tasks, 3, tue, Some(2)));

        let mut v = WeekView::new();
        v.load(data(vec![
            task(1, Some(tue), TaskStatus::Todo),
            task(2, Some(tue), TaskStatus::Todo),
        ]));
        v.update(Message::Press(2));
        v.update(Message::Moved(Point::new(0.0, 0.0)));
        v.update(Message::Moved(Point::new(0.0, 40.0)));
        v.update(Message::Hover(tue, Some(1)));
        let _ = v.view();
        assert_eq!(
            v.update(Message::DropOn(tue, Some(1))),
            Action::Move(2, tue, 0.0)
        );
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
            versions: Vec::new(),
            streaming: None,
            blocked: None,
            empty: false,
            stale: true,
        });
        let week = d.week;
        v.load(d);
        assert_eq!(
            v.update(Message::Panel(report_panel::Message::Retry(
                ReportKind::Plan
            ))),
            Action::Panel(report_panel::Action::Generate(ReportKind::Plan, week))
        );
    }

    #[test]
    fn recommended_tasks_are_ranked_skipping_missing_and_deleted() {
        use fortnight_core::domain::{Report, ReportStatus};
        let mut v = WeekView::new();
        let mut d = data(vec![
            task(1, Some(date(2026, 11, 17)), TaskStatus::Todo),
            task(2, Some(date(2026, 11, 18)), TaskStatus::Dropped),
            task(3, Some(date(2026, 11, 19)), TaskStatus::Done),
        ]);
        let plan = r#"{"focus":"F","tasks":[{"id":2},{"id":9},{"id":3,"day":"2026-11-18","why":"w"},{"id":1}]}"#;
        d.panel.slots.push(report_panel::ReportSlot {
            kind: ReportKind::Plan,
            versions: vec![Report {
                id: 1,
                kind: ReportKind::Plan,
                week_start: d.week.start,
                status: ReportStatus::Ready,
                digest_json: "{}".into(),
                content_md: Some(plan.into()),
                edited_md: None,
                model: None,
                endpoint_host: None,
                prompt_version: 4,
                error: None,
                created_at: jiff::Timestamp::UNIX_EPOCH,
            }],
            streaming: None,
            blocked: None,
            empty: false,
            stale: false,
        });
        v.load(d);
        assert_eq!(v.ranks(), vec![(3, 1), (1, 2)]);
        let _ = v.view();
    }
}
