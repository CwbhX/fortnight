//! The Week tab (PLAN.md 4.2 "Week").
//!
//! One column per working day, in week order, all the same width and
//! height. Each column has a title bar: the heading (weekday and date;
//! click to open the day in the Today tab) and the load bar under it,
//! always shown: estimated tasks take their estimate, tasks without one
//! split what is left of the day evenly, and done tasks' parts are the
//! accent (no text: hovering says "2h of 8h estimated · 3 without a time:
//! 2h each · 2h done"; a warning sign when estimates exceed capacity). Then
//! the day's tasks: open ones first, done ones dimmed after them. A task is
//! placed by `Task::day()` (its planned day). Non-working days
//! share one extra column ("Sat–Sun"), shown only when one of them has
//! tasks. Today's column has the accent ring. Overdue tasks from before the
//! week are on the Today tab only. The whole tab scrolls.
//!
//! The AI part comes from the report panel and is generated automatically
//! (see `report_panel`): the Weekly Summary card, folded until its title
//! bar is clicked. (Recommended tasks are on the Today tab.)
//!
//! Tasks are small cards. An open task's card can be dragged: a copy follows
//! the pointer, the day under it is highlighted, and an accent line shows
//! where it will land. Dropping on a card puts the task before it; dropping
//! on a column's free space or a done card puts it after that day's open
//! tasks. Another day
//! becomes the task's planned day (its deadline stays as it is); within a
//! day only the order (`Task::position`) changes. Dropping on the weekend
//! column's free space uses its first day; letting go anywhere else cancels.
//! The card lands exactly where the highlight and line show when the
//! button goes up (the pointer's last target, kept up to date on every
//! move), and the copy under the pointer lets it through (`inert`).
//! A click on a card opens the task dialog.
//!
//! Skipped on purpose: deleted tasks (the Today tab has them).

use crate::inert::inert;
use crate::motion;
use crate::theme;
use crate::views::panel::{self, LoadLine, load_line, load_view, title_bar, title_bar_alone};
use crate::views::report_panel::{self, ReportPanel, ReportPanelData};
use fortnight_core::domain::{ReportKind, Task, TaskId, TaskStatus, WeekRef};
use fortnight_core::report::{self, DayLoad, fmt_hours};
use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, Stack, button, checkbox, column, container, mouse_area, pin, row, scrollable,
    space, stack, text, tooltip,
};
use iced::{Center, Color, Element, Fill, Point, Theme, mouse};
use jiff::civil::{Date, Weekday};

/// The shortest a day column gets, so an empty week still looks like a
/// week: room for the title bar and about eleven cards.
const MIN_COLUMN_HEIGHT: f32 = 560.0;

#[derive(Debug, Clone)]
pub enum Message {
    /// A task's checkbox.
    Toggle(TaskId, bool),
    /// A day's heading was clicked.
    OpenDay(Date),
    Panel(report_panel::Message),
    /// The mouse went down on a task card.
    Press(TaskId),
    /// The pointer moved while a card is held (position in the tab).
    Moved(Point),
    /// While dragging, the pointer is over a day column's free space
    /// (`None`) or the card of this task (sent on entering and on every
    /// move inside, so it is always where the pointer last was).
    Hover(Date, Option<TaskId>),
    /// While dragging, the pointer left what `Hover` with the same day and
    /// card reported entering.
    Leave(Date, Option<TaskId>),
    /// The mouse button went up: the drag (if any) is over, and the card
    /// lands where the highlight and landing line showed (`Drag::over`).
    EndDrag,
    /// A day's `+`: add a task planned for that day.
    AddTask(Date),
    /// A click on a done task's card (open cards report clicks through
    /// `Press` then `EndDrag` without a drag).
    Edit(TaskId),
    /// The Weekly Summary's title bar: fold or unfold it.
    ToggleSummary,
}

impl Message {
    /// Pointer bookkeeping that changes nothing but the drag: `main.rs`
    /// skips its refresh for these, as they arrive with every mouse move.
    pub fn is_pointer_move(&self) -> bool {
        matches!(
            self,
            Message::Press(_) | Message::Moved(_) | Message::Hover(..) | Message::Leave(..)
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
    /// A card was dragged: plan it for `Date` (if it isn't already its
    /// day) and set its position to the `f64`.
    Move(TaskId, Date, f64),
    /// A card was dragged between tasks whose positions collided: plan it
    /// for `Date` (if needed) and renumber the day's open tasks.
    MoveAndReorder(TaskId, Date, Vec<(TaskId, f64)>),
    /// Open the new-task dialog for this day.
    NewTask(Date),
    /// A card was clicked: open the task dialog to edit it.
    Edit(TaskId),
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
    /// Worked out from `data` by `load`; `view` only reads it.
    layout: Layout,
    panel: ReportPanel,
    drag: Option<Drag>,
    /// Whether the Weekly Summary is unfolded (it starts folded).
    summary_open: bool,
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
        self.layout = Layout::new(&data);
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
            Message::AddTask(day) => Action::NewTask(day),
            Message::Edit(id) => Action::Edit(id),
            Message::ToggleSummary => {
                self.summary_open = !self.summary_open;
                Action::None
            }
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
            // Only if it is still the target: leaving one target and
            // entering the next may arrive in either order.
            Message::Leave(day, card) => {
                if let Some(drag) = &mut self.drag
                    && drag.over == Some((day, card))
                {
                    drag.over = None;
                }
                Action::None
            }
            // A press let go before it became a drag is a click; a drag
            // lands on what the pointer was last over, exactly what the
            // highlight and line show, or nowhere.
            Message::EndDrag => {
                let Some(drag) = self.drag.take() else {
                    return Action::None;
                };
                if !drag.active {
                    return Action::Edit(drag.id);
                }
                let (Some((day, before)), Some(data)) = (drag.over, &self.data) else {
                    return Action::None;
                };
                let Some(task) = data.tasks.iter().find(|t| t.id == drag.id) else {
                    return Action::None;
                };
                drop_action(task, &self.layout.tasks(&data.tasks, day), day, before)
                    .unwrap_or(Action::None)
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
        let dragging = self.dragging();
        // The dragged task, if it still exists.
        let held = dragging.and_then(|d| data.tasks.iter().find(|t| t.id == d.id));

        // Equal heights: the backgrounds are pushed under the contents in a
        // stack, so they take the height of the tallest column (iced's row
        // can't stretch its children to the tallest one inside a scrollable).
        let mut backgrounds = Row::new().spacing(theme::SPACING);
        let mut contents = Row::new().spacing(theme::SPACING);
        for column in &self.layout.columns {
            let spec = &column.spec;
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
                    .on_move(move |_| Message::Hover(day, None))
                    .on_exit(Message::Leave(day, None));
            }
            backgrounds = backgrounds.push(free);
            contents = contents.push(day_column(data, &self.layout, column, dragging, held));
        }
        let grid = column_grid(contents, backgrounds);

        let mut body = Column::new().spacing(theme::SPACING * 2.0).push(grid);
        if self.panel.slot(ReportKind::Summary).is_some() {
            body = body.push(summary_card(
                self.summary_open,
                self.panel.summary_tools().map(|e| e.map(Message::Panel)),
                self.panel
                    .status_view(ReportKind::Summary)
                    .map(|e| e.map(Message::Panel)),
                self.panel.summary_view().map(Message::Panel),
            ));
        }
        let mut layers = stack![body.padding(theme::SPACING * 2.0)];
        if let Some(drag) = dragging
            && let Some(at) = drag.at
            && let Some(task) = held
        {
            let ghost = container(task_card(
                task,
                task.day().unwrap_or(data.today),
                data.today,
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
            // Inert: the pointer is on it, and must reach the targets below.
            layers = layers.push(pin(inert(ghost)).position(Point::new(at.x - 24.0, at.y - 16.0)));
        }
        let mut area = mouse_area(layers);
        if self.drag.is_some() {
            area = area.on_move(Message::Moved).on_release(Message::EndDrag);
        }
        scrollable(area).height(Fill).into()
    }
}

/// The Weekly Summary card. Its heading sits in a tinted bar of its own, in
/// bold with a copy icon beside it and the status at the right, so it
/// doesn't read as one more heading of the Markdown below it. The bar
/// folds and unfolds the body (`open`); the copy icon works either way.
fn summary_card<'a>(
    open: bool,
    tools: Option<Element<'a, Message>>,
    status: Option<Element<'a, Message>>,
    body: Element<'a, Message>,
) -> Element<'a, Message> {
    let mut head = row![
        theme::disclosure(open),
        text("Weekly Summary")
            .size(theme::TEXT_BODY)
            .font(theme::FONT_BOLD)
    ]
    .spacing(theme::SPACING / 2.0)
    .align_y(Center);
    if let Some(tools) = tools {
        head = head.push(tools);
    }
    if let Some(status) = status {
        head = head.push(space::horizontal()).push(status);
    }
    let bar = container(head).padding(
        iced::Padding::from([theme::SPACING * 0.75, theme::SPACING * 1.5]).right(theme::SPACING),
    );
    let bar = if open {
        bar.width(Fill).style(title_bar(theme::RADIUS))
    } else {
        bar.width(Fill).style(title_bar_alone(theme::RADIUS))
    };
    // The copy icon takes its own clicks; the rest of the bar toggles.
    let bar = mouse_area(bar)
        .on_press(Message::ToggleSummary)
        .interaction(mouse::Interaction::Pointer);
    let body = container(body).padding(theme::SPACING * 1.5).width(Fill);
    container(column![bar, motion::fold(open, body)])
        .width(Fill)
        .style(theme::card)
        .into()
}

/// The day columns' contents over their backgrounds, at least
/// `MIN_COLUMN_HEIGHT` tall: the backgrounds stretch to the taller of that
/// and the tallest column. The height comes from a 1 px strut at the left
/// of each row (iced's `Row::push` drops anything 0 px wide), so contents
/// and backgrounds stay lined up.
fn column_grid<'a, M: 'a, R: iced::advanced::Renderer + 'a>(
    contents: Row<'a, M, Theme, R>,
    backgrounds: Row<'a, M, Theme, R>,
) -> Stack<'a, M, Theme, R> {
    let base = row![
        space().width(1).height(MIN_COLUMN_HEIGHT),
        contents.width(Fill)
    ];
    let under = row![space().width(1), backgrounds.width(Fill)];
    stack![base].push_under(under).width(Fill)
}

/// What `view` shows, worked out once per `load` rather than on every
/// frame (`view` runs on every pointer move while dragging, and
/// `WeekData::tasks` holds every task there is).
#[derive(Debug, Default)]
struct Layout {
    /// Each day of the week with its tasks in `by_day` order, as indices
    /// into `WeekData::tasks`.
    days: Vec<(Date, Vec<usize>)>,
    columns: Vec<DayColumn>,
}

impl Layout {
    fn new(data: &WeekData) -> Self {
        let days = by_day(&data.tasks, &data.week.days());
        let columns = columns(data, &days)
            .into_iter()
            .map(|spec| {
                let (name, dates) = heading_text(&spec);
                let open_day = spec
                    .days
                    .iter()
                    .copied()
                    .find(|d| !on(&days, *d).is_empty())
                    .unwrap_or(spec.days[0]);
                let load = load_line(&report::column_load(&data.tasks, &spec.days, &data.loads));
                DayColumn {
                    spec,
                    name,
                    dates,
                    open_day,
                    load,
                }
            })
            .collect();
        Layout { days, columns }
    }

    /// The tasks on `day` (one of the week's), in `by_day` order.
    fn tasks<'a>(&self, tasks: &'a [Task], day: Date) -> Vec<&'a Task> {
        on(&self.days, day).iter().map(|&i| &tasks[i]).collect()
    }
}

/// The indices listed for `day` in `by_day`'s output (none for a day it
/// doesn't list).
fn on(days: &[(Date, Vec<usize>)], day: Date) -> &[usize] {
    days.iter()
        .find(|(d, _)| *d == day)
        .map_or(&[], |(_, tasks)| tasks.as_slice())
}

/// One column as shown: what it lists, its heading and its load bar.
#[derive(Debug, Clone, PartialEq)]
struct DayColumn {
    spec: ColumnSpec,
    /// The heading: weekday name(s), then the date(s) (`heading_text`).
    name: String,
    dates: String,
    /// The day the heading opens: the first one with tasks.
    open_day: Date,
    load: LoadLine,
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
/// non-working column when one of its days has tasks (`days` from
/// `by_day`).
fn columns(data: &WeekData, days: &[(Date, Vec<usize>)]) -> Vec<ColumnSpec> {
    let (working, other): (Vec<Date>, Vec<Date>) = data
        .week
        .days()
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
    if other.iter().any(|d| !on(days, *d).is_empty()) {
        out.push(ColumnSpec {
            is_today: other.contains(&data.today),
            days: other,
            weekend: true,
        });
    }
    out
}

/// For each of `days`, the tasks whose `Task::day()` it is, without
/// dropped ones, as indices into `tasks`: open first, then done; by
/// position.
fn by_day(tasks: &[Task], days: &[Date]) -> Vec<(Date, Vec<usize>)> {
    let mut out: Vec<(Date, Vec<usize>)> = days.iter().map(|d| (*d, Vec::new())).collect();
    for (i, t) in tasks.iter().enumerate() {
        if t.status == TaskStatus::Dropped {
            continue;
        }
        if let Some(day) = t.day()
            && let Some((_, list)) = out.iter_mut().find(|(d, _)| *d == day)
        {
            list.push(i);
        }
    }
    for (_, list) in &mut out {
        list.sort_by(|&a, &b| {
            let (a, b) = (&tasks[a], &tasks[b]);
            (!a.status.is_open())
                .cmp(&!b.status.is_open())
                .then(a.position.total_cmp(&b.position))
                .then(a.id.cmp(&b.id))
        });
    }
    out
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

/// The small grey line under a task title: estimate and project.
fn meta_line(task: &Task) -> String {
    let mut parts = Vec::new();
    if let Some(m) = task.estimate_minutes {
        parts.push(fmt_hours(m));
    }
    if let Some(p) = &task.project {
        parts.push(format!("#{p}"));
    }
    parts.join(" · ")
}

/// The deadline at a card's bottom right ("Wed 18"), when the task is
/// planned for another day than its deadline.
fn due_corner(task: &Task, column_day: Date) -> Option<String> {
    task.due_on
        .filter(|due| *due != column_day)
        .map(|due| due.strftime("%a %-d").to_string())
}

/// A column: its title bar over its cards.
fn day_column<'a>(
    data: &'a WeekData,
    layout: &Layout,
    column: &DayColumn,
    drag: Option<&Drag>,
    held: Option<&Task>,
) -> Element<'a, Message> {
    let list = card_list(data, layout, &column.spec, drag, held);
    container(column![
        column_title(column),
        container(Column::new().spacing(8).width(Fill).push(list)).padding(theme::SPACING)
    ])
    .padding(COLUMN_RING)
    .width(Fill)
    .into()
}

/// A column's title bar: the heading (opens the day), the `+` and the load
/// bar.
fn column_title<'a>(column: &DayColumn) -> Element<'a, Message> {
    let spec = &column.spec;
    let name_style: fn(&Theme) -> text::Style = if spec.is_today {
        theme::accent
    } else {
        text::default
    };
    let heading = button(
        row![
            text(column.name.clone())
                .size(theme::TEXT_BODY)
                .style(name_style)
                .font(theme::FONT_BOLD)
                .wrapping(Wrapping::None),
            text(column.dates.clone())
                .size(theme::TEXT_SMALL)
                .style(theme::muted)
                .wrapping(Wrapping::None),
        ]
        .spacing(6)
        .align_y(Center),
    )
    .padding(0)
    .width(Fill)
    .style(theme::text_button)
    .on_press(Message::OpenDay(column.open_day));
    // `+` in the top right: a new task for the column's (first) day.
    let add = tooltip(
        button(theme::icon(theme::Icon::Plus))
            .padding(2)
            .style(theme::icon_button)
            .on_press(Message::AddTask(spec.days[0])),
        theme::tip("Add a task"),
        tooltip::Position::Bottom,
    );
    // The heading sits in a title bar like the Weekly Summary's, inset by
    // today's ring so the ring stays visible around it.
    let bar = Column::new()
        .spacing(6)
        .push(row![heading, add].align_y(Center))
        .push(load_view(&column.load));
    container(bar)
        .padding([theme::SPACING * 0.6, theme::SPACING])
        .width(Fill)
        .style(title_bar(theme::RADIUS - COLUMN_RING))
        .into()
}

/// A column's cards, day by day (with a small heading per day in the
/// shared non-working column), or "Nothing planned". `held` is the dragged
/// task.
fn card_list<'a>(
    data: &'a WeekData,
    layout: &Layout,
    spec: &ColumnSpec,
    drag: Option<&Drag>,
    held: Option<&Task>,
) -> Element<'a, Message> {
    let dragged = drag.map(|d| d.id);
    let over = drag.and_then(|d| d.over);
    // Cards sit on fixed 8 px slots that double as the landing line, so
    // showing the line never moves anything (which would move the target
    // under the pointer and make the line jump back and forth).
    let mut list = Column::new().width(Fill);
    let mut any = false;
    // The line shows only where a drop would change something, and only
    // among other open tasks (not in an empty day).
    let shows_line = |day: Date, tasks: &[&Task], before: Option<TaskId>| {
        over == Some((day, before))
            && held.is_some_and(|held| line_wanted(held, tasks, day, before))
    };
    for day in &spec.days {
        let tasks = layout.tasks(&data.tasks, *day);
        if tasks.is_empty() {
            continue;
        }
        if spec.weekend && spec.days.len() > 1 {
            list = list.push(theme::section_heading(day.strftime("%a %-d").to_string()));
        }
        let open = tasks.iter().filter(|t| t.status.is_open()).count();
        // Done cards are targets for "after the open tasks" too, so that
        // drop isn't left to the thin gap above them.
        let at_end = shows_line(*day, &tasks, None)
            || tasks[open..]
                .iter()
                .any(|t| shows_line(*day, &tasks, Some(t.id)));
        for (i, task) in tasks.iter().enumerate() {
            any = true;
            list = list.push(task_row(
                task,
                *day,
                data.today,
                dragged.is_some().then_some(dragged == Some(task.id)),
                i < open && shows_line(*day, &tasks, Some(task.id)),
            ));
            if i + 1 == open {
                list = list.push(slot(at_end));
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
    list.into()
}

/// Width of today's accent ring (`theme::day_column`); the title bar sits
/// inside it.
const COLUMN_RING: f32 = 1.5;

/// A task card in a day column: draggable when open. `dragging` is `None`
/// when nothing is being dragged, else whether this is the dragged card
/// (shown faded). While dragging, an open card is a drop target: the
/// dragged task goes before it.
fn task_row<'a>(
    task: &'a Task,
    day: Date,
    today: Date,
    dragging: Option<bool>,
    line_above: bool,
) -> Element<'a, Message> {
    let id = task.id;
    let card = column![
        slot(line_above),
        task_card(task, day, today, dragging == Some(true))
    ];
    if !task.status.is_open() {
        let mut area = mouse_area(card)
            .on_press(Message::Edit(task.id))
            .interaction(mouse::Interaction::Pointer);
        if dragging.is_some() {
            // Dropping here means after the day's open tasks
            // (`drop_position` puts a `before` that isn't open at the end).
            // Its own target, not the column's: entering it and leaving the
            // free space may arrive in either order.
            area = area
                .on_enter(Message::Hover(day, Some(task.id)))
                .on_exit(Message::Leave(day, Some(task.id)))
                .on_move(move |_| Message::Hover(day, Some(id)));
        }
        return area.into();
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
            .on_exit(Message::Leave(day, Some(task.id)))
            .on_move(move |_| Message::Hover(day, Some(id)));
    }
    area.into()
}

/// Whether dropping `task` on `day` before `before` deserves the landing
/// line: another open task is there to order against, and the drop would
/// change something. `on_day` is the day's tasks (`Layout::tasks`).
fn line_wanted(task: &Task, on_day: &[&Task], day: Date, before: Option<TaskId>) -> bool {
    on_day.iter().any(|t| t.status.is_open() && t.id != task.id)
        && drop_position(task, on_day, day, before).is_some()
}

/// The 8 px gap above a card (and after a day's open cards): empty, or the
/// accent line that shows where a dragged card will land.
fn slot<'a>(line: bool) -> Element<'a, Message> {
    let inner: Element<'a, Message> = if line {
        container(space())
            .width(Fill)
            .height(3)
            .style(|_: &Theme| container::Style {
                background: Some(theme::accent_color().into()),
                border: iced::border::rounded(2),
                ..container::Style::default()
            })
            .into()
    } else {
        space().into()
    };
    container(inner).width(Fill).height(8).center_y(8).into()
}

/// What dropping `task` on `day`, before the open task `before` (or after
/// the day's open tasks), should do: `Move` with a midpoint position when
/// there is room between its neighbours, or `MoveAndReorder` renumbering
/// the day's open tasks when adjacent positions collide. `None` when nothing
/// would change (same day, same place).
fn drop_action(task: &Task, on_day: &[&Task], day: Date, before: Option<TaskId>) -> Option<Action> {
    let id = task.id;
    let open: Vec<&Task> = on_day
        .iter()
        .copied()
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
    match (index.checked_sub(1).map(|i| others[i]), others.get(index)) {
        (Some(prev), Some(next)) => {
            let mid = (prev.position + next.position) / 2.0;
            if prev.position < mid && mid < next.position {
                Some(Action::Move(id, day, mid))
            } else {
                let mut order: Vec<TaskId> = others.iter().map(|t| t.id).collect();
                order.insert(index, id);
                let positions = order
                    .into_iter()
                    .enumerate()
                    .map(|(i, tid)| (tid, (i + 1) as f64))
                    .collect();
                Some(Action::MoveAndReorder(id, day, positions))
            }
        }
        (None, Some(next)) => Some(Action::Move(id, day, next.position - 1.0)),
        (Some(prev), None) => Some(Action::Move(id, day, prev.position + 1.0)),
        (None, None) => Some(Action::Move(id, day, task.position)),
    }
}

/// Where `task` goes when dropped on `day`, before the open task `before`
/// (or after the day's open tasks): its new `position`, or `None` when
/// nothing would change.
fn drop_position(task: &Task, on_day: &[&Task], day: Date, before: Option<TaskId>) -> Option<f64> {
    match drop_action(task, on_day, day, before)? {
        Action::Move(_, _, pos) => Some(pos),
        Action::MoveAndReorder(id, _, positions) => positions
            .into_iter()
            .find(|(tid, _)| *tid == id)
            .map(|(_, p)| p),
        _ => None,
    }
}

/// The card itself: checkbox, priority, title and the meta line.
fn task_card<'a>(task: &'a Task, day: Date, today: Date, faded: bool) -> Element<'a, Message> {
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

    let title = Row::new().spacing(4).push(theme::title_with_marks(
        task.title.as_str(),
        if done { 0 } else { task.priority },
        false,
        title_style,
        |t| {
            t.size(theme::TEXT_SMALL)
                .wrapping(Wrapping::WordOrGlyph)
                .width(Fill)
        },
    ));

    let mut body = Column::new().spacing(2).width(Fill).push(title);
    if !done {
        let meta = meta_line(task);
        let due = due_corner(task, day);
        if !meta.is_empty() || due.is_some() {
            // Estimate and project on the left, the deadline bottom right.
            let mut line = row![
                text(meta)
                    .size(11)
                    .style(theme::muted)
                    .wrapping(Wrapping::WordOrGlyph)
                    .width(Fill)
            ]
            .spacing(6)
            .align_y(iced::Bottom);
            if let Some(due) = due {
                line = line.push(
                    text(due)
                        .size(11)
                        .style(theme::due_style(overdue))
                        .wrapping(Wrapping::None),
                );
            }
            body = body.push(line);
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
    let mut fill = panel::card_fill(theme, done, false);
    if faded {
        fill = fill.scale_alpha(0.35);
    }
    container::Style {
        background: Some(fill.into()),
        text_color: faded.then(|| p.text.scale_alpha(0.4)),
        border: iced::border::rounded(panel::CARD_RADIUS),
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
            background: Some(theme::accent_color().scale_alpha(0.08).into()),
            border: iced::Border {
                color: theme::accent_color(),
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

    /// A week with little planned still gets tall columns, and a column
    /// taller than that stretches every background to it.
    #[test]
    fn columns_have_a_minimum_height() {
        use iced::advanced::layout::Limits;
        use iced::advanced::widget::Tree;
        let grid = |content: f32| {
            let contents: Row<'_, (), Theme, ()> = Row::new()
                .push(container(space().height(content)).width(Fill))
                .push(container(space().height(10)).width(Fill));
            let backgrounds = Row::new()
                .push(container(space()).width(Fill).height(Fill))
                .push(container(space()).width(Fill).height(Fill));
            let mut el: iced::Element<'_, (), Theme, ()> =
                column_grid(contents, backgrounds).into();
            let mut tree = Tree::new(&el);
            let limits = Limits::new(iced::Size::ZERO, iced::Size::new(1000.0, f32::INFINITY));
            let node = el.as_widget_mut().layout(&mut tree, &(), &limits);
            let under = &node.children()[0].children()[1];
            let heights: Vec<f32> = under.children().iter().map(|c| c.size().height).collect();
            (node.size().height, heights)
        };
        assert_eq!(grid(50.0), (MIN_COLUMN_HEIGHT, vec![MIN_COLUMN_HEIGHT; 2]));
        let tall = MIN_COLUMN_HEIGHT + 140.0;
        assert_eq!(grid(tall), (tall, vec![tall; 2]));
    }
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
                slots: Vec::new(),
            },
        }
    }

    fn load(date: Date, capacity: u32) -> DayLoad {
        DayLoad {
            date,
            load_minutes: 0,
            capacity_minutes: capacity,
            unestimated: 0,
        }
    }

    fn specs(d: &WeekData) -> Vec<ColumnSpec> {
        columns(d, &by_day(&d.tasks, &d.week.days()))
    }

    fn on_day(tasks: &[Task], day: Date) -> Vec<&Task> {
        by_day(tasks, &[day])[0]
            .1
            .iter()
            .map(|&i| &tasks[i])
            .collect()
    }

    /// `drop_position` as the view calls it, from all the tasks.
    fn drop_at(tasks: &[Task], id: TaskId, day: Date, before: Option<TaskId>) -> Option<f64> {
        let task = tasks.iter().find(|t| t.id == id)?;
        drop_position(task, &on_day(tasks, day), day, before)
    }

    fn line_at(tasks: &[Task], id: TaskId, day: Date, before: Option<TaskId>) -> bool {
        let task = tasks.iter().find(|t| t.id == id).unwrap();
        line_wanted(task, &on_day(tasks, day), day, before)
    }

    /// The load line of a one-day column with 8h of capacity.
    fn line_of(tasks: &[Task], d: Date) -> LoadLine {
        load_line(&report::column_load(tasks, &[d], &[load(d, 480)]))
    }

    #[test]
    fn tasks_are_bucketed_by_planned_day_open_first() {
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
        let ids: Vec<_> = on_day(&tasks, d).iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![2, 3, 7, 1]);
        let ids: Vec<_> = on_day(&tasks, date(2026, 11, 19))
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![4, 5]);
        // A day the buckets don't list has nothing.
        let buckets = by_day(&tasks, &[d]);
        assert!(on(&buckets, date(2026, 11, 19)).is_empty());
    }

    #[test]
    fn columns_are_the_working_days_and_mark_today() {
        let cols = specs(&data(vec![]));
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
        let cols = specs(&data(vec![
            task(1, Some(date(2026, 11, 22)), TaskStatus::Dropped),
            task(2, Some(date(2026, 11, 29)), TaskStatus::Todo),
        ]));
        assert_eq!(cols.len(), 5);

        let cols = specs(&data(vec![task(
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
        let cols = specs(&data(vec![]));
        assert_eq!(
            heading_text(&cols[0]),
            ("Mon".to_string(), "16 Nov".to_string())
        );
    }

    #[test]
    fn untimed_tasks_share_what_the_estimates_leave() {
        let d = date(2026, 11, 18);
        let est = |id, status, m| {
            let mut t = task(id, Some(d), status);
            t.estimate_minutes = m;
            t
        };
        // Four tasks without a time: a quarter of the day each.
        let four: Vec<Task> = (1..=4).map(|i| est(i, TaskStatus::Todo, None)).collect();
        let line = line_of(&four, d);
        assert_eq!(line.fraction, 1.0);
        assert_eq!(line.done_fraction, 0.0);
        assert_eq!(
            line.figures,
            "0m of 8h estimated · 4 without a time: 2h each"
        );

        // Two 2h tasks and two without a time: 25% each; one of each done.
        let mixed = [
            est(1, TaskStatus::Done, Some(120)),
            est(2, TaskStatus::Todo, Some(120)),
            est(3, TaskStatus::Done, None),
            est(4, TaskStatus::Todo, None),
        ];
        let line = line_of(&mixed, d);
        assert_eq!(line.fraction, 1.0);
        assert_eq!(line.done_fraction, 0.5);
        assert_eq!(line.over_by, None);

        // Only timed tasks: their share, nothing more.
        let line = line_of(&mixed[..2], d);
        assert_eq!(line.fraction, 0.5);
        assert_eq!(line.done_fraction, 0.25);
        assert_eq!(line.figures, "4h of 8h estimated · 2h done");

        // Estimates over capacity: untimed tasks get nothing, the bar is
        // full and the warning shows.
        let over = [
            est(1, TaskStatus::Todo, Some(540)),
            est(2, TaskStatus::Done, None),
        ];
        let line = line_of(&over, d);
        assert_eq!(line.fraction, 1.0);
        assert_eq!(line.done_fraction, 0.0);
        assert_eq!(line.over_by, Some(60));
        assert!(line.figures.contains("1 without a time: no time left"));
        assert!(line.figures.ends_with("over by 1h"));

        // No tasks: an empty bar.
        let line = line_of(&[], d);
        assert_eq!((line.fraction, line.done_fraction), (0.0, 0.0));
        assert_eq!(line.figures, "0m of 8h estimated");
    }

    #[test]
    fn the_weekend_column_borrows_a_working_days_capacity() {
        // Wednesday has 7h; Saturday and Sunday none. Only Saturday has a
        // task, so the column gets one working day's capacity.
        let sat = date(2026, 11, 21);
        let mut d = data(vec![task(1, Some(sat), TaskStatus::Todo)]);
        d.loads = vec![
            load(date(2026, 11, 18), 420),
            load(sat, 0),
            load(date(2026, 11, 22), 0),
        ];
        let layout = Layout::new(&d);
        let weekend = layout.columns.last().unwrap();
        assert!(weekend.spec.weekend);
        assert_eq!(
            weekend.load.figures,
            "0m of 7h estimated · 1 without a time: 7h each"
        );
        assert_eq!(weekend.open_day, sat);
        assert_eq!(
            (weekend.name.as_str(), weekend.dates.as_str()),
            ("Sat to Sun", "21 to 22 Nov")
        );
    }

    #[test]
    fn meta_line_shows_estimate_and_project_and_the_corner_another_deadline() {
        let d = date(2026, 11, 18);
        let mut t = task(1, Some(date(2026, 11, 20)), TaskStatus::Doing);
        t.planned_for = Some(d);
        t.estimate_minutes = Some(120);
        t.project = Some("work".into());
        assert_eq!(meta_line(&t), "2h · #work");
        assert_eq!(due_corner(&t, d).as_deref(), Some("Fri 20"));
        let plain = task(2, Some(d), TaskStatus::Todo);
        assert_eq!(meta_line(&plain), "");
        assert_eq!(due_corner(&plain, d), None);
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
        v.update(Message::Hover(wed, None));
        assert_eq!(v.update(Message::EndDrag), Action::Edit(1));

        v.update(Message::Press(1));
        v.update(Message::Moved(Point::new(10.0, 10.0)));
        v.update(Message::Moved(Point::new(80.0, 12.0)));
        assert!(v.dragging().is_some());
        v.update(Message::Hover(wed, None));
        let _ = v.view();
        assert_eq!(v.update(Message::EndDrag), Action::Move(1, wed, 1.0));
        assert!(v.drag.is_none());

        // A press let go without moving is a click: edit the task.
        v.update(Message::Press(1));
        v.update(Message::Moved(Point::new(5.0, 5.0)));
        assert_eq!(v.update(Message::EndDrag), Action::Edit(1));
        assert_eq!(v.update(Message::EndDrag), Action::None);

        // Let go outside a column: cancelled.
        v.update(Message::Press(1));
        v.update(Message::Moved(Point::new(0.0, 0.0)));
        v.update(Message::Moved(Point::new(50.0, 0.0)));
        v.update(Message::EndDrag);
        assert!(v.drag.is_none());
        assert!(Message::Moved(Point::ORIGIN).is_pointer_move());
        assert!(!Message::EndDrag.is_pointer_move());
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
        assert_eq!(drop_at(&tasks, 3, tue, Some(1)), Some(0.0));
        // Between two.
        assert_eq!(drop_at(&tasks, 3, tue, Some(2)), Some(1.5));
        // To the end of the open ones.
        assert_eq!(drop_at(&tasks, 1, tue, None), Some(4.0));
        // On a done card: the same.
        assert_eq!(drop_at(&tasks, 1, tue, Some(4)), Some(4.0));
        assert_eq!(drop_at(&tasks, 3, tue, Some(4)), None);
        // Already there: nothing.
        assert_eq!(drop_at(&tasks, 3, tue, None), None);
        assert_eq!(drop_at(&tasks, 2, tue, Some(3)), None);
        assert_eq!(drop_at(&tasks, 2, tue, Some(2)), None);
        // Another day: before its card, or alone in an empty day.
        assert_eq!(drop_at(&tasks, 1, wed, Some(5)), Some(4.0));
        assert_eq!(drop_at(&tasks, 1, wed, None), Some(6.0));
        assert_eq!(drop_at(&tasks, 1, date(2026, 11, 19), None), Some(1.0));
        tasks.clear();
        assert_eq!(drop_at(&tasks, 1, tue, None), None);

        // The landing line: not in an empty day, not for a day's only
        // task, not where nothing would change.
        let tasks = vec![
            task(1, Some(tue), TaskStatus::Todo),
            task(2, Some(tue), TaskStatus::Todo),
            task(3, Some(wed), TaskStatus::Todo),
        ];
        assert!(!line_at(&tasks, 1, date(2026, 11, 19), None));
        assert!(!line_at(&tasks, 3, wed, None));
        assert!(!line_at(&tasks, 2, tue, None));
        assert!(line_at(&tasks, 1, tue, None));
        assert!(line_at(&tasks, 2, tue, Some(1)));
        assert!(line_at(&tasks, 3, tue, Some(2)));

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
        assert_eq!(v.update(Message::EndDrag), Action::Move(2, tue, 0.0));

        // Adjacent tasks sharing the same position trigger a renumber instead
        // of computing a colliding midpoint.
        let mut a = task(1, Some(tue), TaskStatus::Todo);
        let mut b = task(2, Some(tue), TaskStatus::Todo);
        let mut c = task(3, Some(tue), TaskStatus::Todo);
        a.position = 1.0;
        b.position = 1.0;
        c.position = 2.0;
        let on_day = [&a, &b, &c];
        assert_eq!(
            drop_action(&c, &on_day, tue, Some(2)),
            Some(Action::MoveAndReorder(
                3,
                tue,
                vec![(1, 1.0), (3, 2.0), (2, 3.0)]
            ))
        );
    }

    #[test]
    fn leaving_the_target_clears_it() {
        let (tue, wed) = (date(2026, 11, 17), date(2026, 11, 18));
        let mut v = WeekView::new();
        v.load(data(vec![
            task(1, Some(tue), TaskStatus::Todo),
            task(2, Some(tue), TaskStatus::Todo),
        ]));
        v.update(Message::Press(2));
        v.update(Message::Moved(Point::new(0.0, 0.0)));
        v.update(Message::Moved(Point::new(0.0, 40.0)));
        let over = |v: &WeekView| v.drag.as_ref().and_then(|d| d.over);

        // Onto card 1, then off it into a gap: no target.
        v.update(Message::Hover(tue, Some(1)));
        v.update(Message::Leave(tue, Some(1)));
        assert_eq!(over(&v), None);

        // From the free space onto a card: the enter may come before the
        // exit; either way the card stays the target.
        v.update(Message::Hover(tue, None));
        v.update(Message::Hover(tue, Some(1)));
        v.update(Message::Leave(tue, None));
        assert_eq!(over(&v), Some((tue, Some(1))));
        v.update(Message::Leave(tue, Some(1)));
        v.update(Message::Hover(wed, None));
        assert_eq!(over(&v), Some((wed, None)));
        let _ = v.view();

        // Let go with no target: only `EndDrag` arrives, nothing moves.
        v.update(Message::Leave(wed, None));
        assert_eq!(over(&v), None);
        assert_eq!(v.update(Message::EndDrag), Action::None);
        assert!(v.drag.is_none());

        // Without a drag it does nothing.
        assert_eq!(v.update(Message::Leave(tue, None)), Action::None);
        assert!(Message::Leave(tue, None).is_pointer_move());
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
            v.update(Message::AddTask(date(2026, 11, 18))),
            Action::NewTask(date(2026, 11, 18))
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
            kind: ReportKind::Summary,
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
                ReportKind::Summary
            ))),
            Action::Panel(report_panel::Action::Generate(ReportKind::Summary, week))
        );
        let _ = v.view();
    }
}
