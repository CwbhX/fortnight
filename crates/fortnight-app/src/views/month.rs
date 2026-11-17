//! The Month tab: a calendar grid for seeing and navigating, not editing.
//! Owner: agent O (round 2). PLAN.md section 4.2 "Month".
//!
//! A header of weekday names from `week_start`, then one row per week from
//! `month_weeks`. Each row starts with its week label ("W47"), which opens
//! the week in the Week tab and carries an accent dot when the week has a
//! ready summary. Each day cell shows the date and up to three tasks as
//! small cards like the Week tab's (open first, with the priority `!`; done
//! dimmed; overdue red), then "+N more". Deleted (`Dropped`) tasks are
//! left out, as in the Week tab.
//! Clicking a cell opens the day in the Today tab.
//!
//! A day with tasks due on it (`Task::due_on`, deleted ones left out) shows
//! at its top right the share of them that are done ("67%"); hovering says
//! how many are due and done. Clicking it opens a small dropdown under it
//! listing the tasks due that day as tiny cards, open ones first and done
//! ones at the bottom; clicking anywhere else closes it.
//!
//! The grid fills the content area with equal cells. When the rows get too
//! short for three titles it shows fewer (the "+N more" grows); below one
//! title per cell it keeps a minimum height and scrolls.
//!
//! Titles are cut to the card's width with "…", measured with the app's
//! text engine (the priority marks keep their own measured room), so a
//! card uses its full width whatever the letters.
//!
//! Skipped (prototype scope): keyboard navigation inside the grid and hover
//! tooltips with full titles.

use crate::theme;
use fortnight_core::domain::{Task, TaskStatus, WeekRef};
use fortnight_core::week::month_weeks;
use iced::border::{self, Border};
use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, Space, button, column, container, mouse_area, pin, responsive, row, scrollable,
    space, stack, text, tooltip,
};
use iced::{Element, Fill, Font, Length, Point, Size, Theme};
use jiff::ToSpan;
use jiff::civil::{Date, Weekday};

#[derive(Debug, Clone)]
pub enum Message {
    DayClicked(Date),
    WeekClicked(WeekRef),
    /// A day's completion badge: open its dropdown (or close it if open).
    ToggleDue(Date),
    /// A click outside the open dropdown.
    CloseDue,
    /// A click inside the open dropdown: keeps it open.
    Ignore,
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
    /// The day whose due-tasks dropdown is open.
    open_due: Option<Date>,
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
        if self.data.as_ref().map(|d| d.month) != Some(data.month) {
            self.open_due = None;
        }
        self.data = Some(data);
    }

    pub fn update(&mut self, message: Message) -> Action {
        match message {
            Message::DayClicked(d) => {
                self.open_due = None;
                Action::OpenDay(d)
            }
            Message::WeekClicked(w) => {
                self.open_due = None;
                Action::OpenWeek(w)
            }
            Message::ToggleDue(d) => {
                self.open_due = (self.open_due != Some(d)).then_some(d);
                Action::None
            }
            Message::CloseDue => {
                self.open_due = None;
                Action::None
            }
            Message::Ignore => Action::None,
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let Some(data) = &self.data else {
            return text("Loading…").style(theme::muted).into();
        };
        let open = self.open_due;
        responsive(move |size| grid(data, open, size)).into()
    }
}

// ------------------------------------------------------------------ layout

/// Most task titles a cell lists.
const MAX_TITLES: usize = 3;
/// Width of the week-label column.
const LABEL_WIDTH: f32 = 52.0;
/// Gap between cells and rows.
const GAP: f32 = 6.0;
/// Padding around the whole grid: sides, then top and bottom.
const OUTER_PAD_X: f32 = theme::SPACING * 1.5;
const OUTER_PAD_Y: f32 = theme::SPACING;
/// Padding inside a cell: sides, then top and bottom.
const CELL_PAD_X: f32 = 6.0;
const CELL_PAD_Y: f32 = 5.0;
/// Padding inside a task card: sides, then top and bottom.
const PILL_PAD_X: f32 = 7.0;
const PILL_PAD_Y: f32 = 2.0;
/// Space between the date and the first card.
const DATE_GAP: f32 = 3.0;
/// Text size of the date number.
const DATE_SIZE: f32 = theme::TEXT_SMALL;
/// Text size of task titles and "+N more".
const TITLE_SIZE: f32 = 12.0;
/// Line height factor for all text in the grid.
const LINE: f32 = 1.25;
/// Space between task cards.
const LINE_GAP: f32 = 2.0;
/// Height of the weekday header row (text plus the gap under it).
const HEADER_HEIGHT: f32 = DATE_SIZE * LINE + GAP;

/// Height of one task card line, with the gap above it.
const LINE_HEIGHT: f32 = TITLE_SIZE * LINE + PILL_PAD_Y * 2.0 + LINE_GAP;

/// Height of a cell that shows the date and `lines` title lines.
fn cell_height(lines: usize) -> f32 {
    CELL_PAD_Y * 2.0 + DATE_SIZE * LINE + DATE_GAP + lines as f32 * LINE_HEIGHT
}

/// How many title lines (including a "+N more" line) fit in a row of
/// `height`.
fn lines_that_fit(height: f32) -> usize {
    let free = height - cell_height(0);
    if free <= 0.0 {
        0
    } else {
        (free / LINE_HEIGHT).floor() as usize
    }
}

/// How far a title may run into a card's right padding ([`PILL_PAD_X`]).
const TITLE_OVERHANG: f32 = 4.0;
/// Width of the due-tasks dropdown.
const DROPDOWN_WIDTH: f32 = 230.0;
/// Text size of the completion badge.
const BADGE_SIZE: f32 = 11.0;

/// The grid for `size` (the content area below the top bar), with the
/// due-tasks dropdown of `open` over it.
fn grid(data: &MonthData, open: Option<Date>, size: Size) -> Element<'_, Message> {
    let weeks = month_weeks(data.month, data.week_start);
    let rows = weeks.len().max(1) as f32;

    // Equal rows filling the height; never so short that a cell can't show
    // one title plus "+N more" (then the rows scroll).
    let available = size.height - OUTER_PAD_Y * 2.0 - HEADER_HEIGHT - GAP * (rows - 1.0);
    let min_height = cell_height(2);
    let row_height = (available / rows).max(min_height).floor();
    let scrolls = row_height * rows > available + 0.5;
    let lines = lines_that_fit(row_height).min(MAX_TITLES + 1);

    let cell_width = (size.width - OUTER_PAD_X * 2.0 - LABEL_WIDTH - GAP * 7.0) / 7.0;
    // Room for a title (and its marks) inside a card. It may run a few
    // pixels into the card's right padding, so one more letter fits rather
    // than leaving a visible gap before the marks.
    let max_width = cell_width - CELL_PAD_X * 2.0 - PILL_PAD_X * 2.0 + TITLE_OVERHANG;

    let mut header = Row::new()
        .spacing(GAP)
        .push(Space::new().width(LABEL_WIDTH));
    for day in weekday_order(data.week_start) {
        header = header.push(
            container(text(weekday_abbr(day)).size(DATE_SIZE).style(theme::muted))
                .padding([0.0, CELL_PAD_X])
                .width(Fill),
        );
    }

    let mut body = Column::new().spacing(GAP);
    // Where the open dropdown goes, in the body's coordinates.
    let mut dropdown = None;
    for (r, week) in weeks.iter().enumerate() {
        let mut line = Row::new()
            .spacing(GAP)
            .height(row_height)
            .push(week_label(*week, has_summary(data, *week)));
        for (c, day) in week.days().into_iter().enumerate() {
            line = line.push(day_cell(data, day, lines, max_width));
            if open == Some(day) {
                let due = due_tasks(&data.tasks, day);
                if !due.is_empty() {
                    let right = LABEL_WIDTH + GAP + (c as f32 + 1.0) * (cell_width + GAP) - GAP;
                    let top = r as f32 * (row_height + GAP) + CELL_PAD_Y + DATE_SIZE * LINE + 2.0;
                    dropdown = Some((due, right, top));
                }
            }
        }
        body = body.push(line);
    }

    let body: Element<'_, Message> = match dropdown {
        None => body.into(),
        Some((due, right, top)) => {
            let height = rows * row_height + GAP * (rows - 1.0);
            let estimate = dropdown_height(due.len());
            let x = (right - DROPDOWN_WIDTH).max(0.0);
            // Under the badge; moved up when it would run off the bottom.
            let y = top.min(height - estimate).max(0.0);
            let panel = mouse_area(due_dropdown(due, data.today)).on_press(Message::Ignore);
            stack![
                body,
                mouse_area(space().width(Fill).height(Fill)).on_press(Message::CloseDue),
                pin(panel).position(Point::new(x, y)),
            ]
            .into()
        }
    };

    let body: Element<'_, Message> = if scrolls {
        scrollable(body).height(Fill).into()
    } else {
        body
    };

    column![container(header).height(HEADER_HEIGHT), body]
        .padding([OUTER_PAD_Y, OUTER_PAD_X])
        .width(Fill)
        .height(Fill)
        .into()
}

/// The clickable week label at the start of a row, with the summary dot.
fn week_label<'a>(week: WeekRef, summary: bool) -> Element<'a, Message> {
    let mut label = row![text(format!("W{}", week_number(week))).size(DATE_SIZE)]
        .spacing(4)
        .align_y(iced::Alignment::Center);
    if summary {
        label = label.push(
            container(Space::new().width(6).height(6)).style(|_: &Theme| container::Style {
                background: Some(theme::accent_color().into()),
                border: border::rounded(3),
                ..container::Style::default()
            }),
        );
    }
    button(label)
        .padding([CELL_PAD_Y, 4.0])
        .width(LABEL_WIDTH)
        .style(theme::text_button)
        .on_press(Message::WeekClicked(week))
        .into()
}

/// One day of the grid.
fn day_cell(data: &MonthData, day: Date, lines: usize, max_width: f32) -> Element<'_, Message> {
    let kind = theme::Cell {
        today: day == data.today,
        weekend: !data.working_days.contains(&day.weekday()),
        outside: day.month() != data.month.month() || day.year() != data.month.year(),
    };
    let content = cell_content(&data.tasks, day, data.today, lines);

    let date_label = if day.day() == 1 {
        format!("{} {}", day.day(), month_abbr(day))
    } else {
        day.day().to_string()
    };
    let mut date = text(date_label).size(DATE_SIZE).line_height(LINE);
    if kind.today {
        date = date.style(theme::accent).font(Font {
            weight: iced::font::Weight::Semibold,
            ..theme::FONT
        });
    }
    let mut top = row![date, space::horizontal()].align_y(iced::Alignment::Center);
    if let Some(done) = due_done(&data.tasks, day) {
        top = top.push(completion_badge(day, done, data.today));
    }

    let mut cards = Column::new().spacing(LINE_GAP);
    for t in &content.titles {
        cards = cards.push(task_pill(t, max_width));
    }
    if content.more > 0 {
        cards = cards.push(
            container(
                text(format!("+{} more", content.more))
                    .size(TITLE_SIZE)
                    .line_height(LINE)
                    .wrapping(Wrapping::None)
                    .style(theme::muted),
            )
            .padding([PILL_PAD_Y, PILL_PAD_X]),
        );
    }
    let col = column![top, cards].spacing(DATE_GAP);

    button(
        container(col)
            .padding([CELL_PAD_Y, CELL_PAD_X])
            .width(Fill)
            .height(Fill)
            .clip(true),
    )
    .padding(0)
    .width(Fill)
    .height(Length::Fill)
    .style(cell_button(kind))
    .on_press(Message::DayClicked(day))
    .into()
}

/// One task as a small card, like the Week tab's: the title, then the
/// priority `!` in its colour. Overdue is red on a red tint; done or dropped is
/// faint on a fainter fill.
fn task_pill<'a>(t: &TitleLine, max_width: f32) -> Element<'a, Message> {
    let (done, overdue) = (t.done, t.overdue);
    let priority = if done { 0 } else { t.priority };
    // The marks keep their own measured width; the title gets the rest.
    let marks = match theme::priority_marks(priority) {
        m if m.is_empty() => 0.0,
        m => measure(&m),
    };
    let line = theme::title_with_marks(
        fit_cached(&t.title, max_width - marks),
        priority,
        move |theme: &Theme| text::Style {
            color: Some(if overdue {
                theme::danger_color(theme)
            } else if done {
                theme.palette().text.scale_alpha(0.4)
            } else {
                theme.palette().text
            }),
        },
        |r| {
            r.size(TITLE_SIZE)
                .line_height(LINE)
                .wrapping(Wrapping::None)
        },
    );
    container(line)
        .padding([PILL_PAD_Y, PILL_PAD_X])
        .width(Fill)
        .clip(true)
        .style(move |theme: &Theme| {
            let fill = if overdue {
                theme::danger_color(theme).scale_alpha(0.14)
            } else {
                theme::tint(theme, if done { 0.06 } else { 0.11 })
            };
            container::Style {
                background: Some(fill.into()),
                border: border::rounded(5),
                ..container::Style::default()
            }
        })
        .into()
}

/// The "67%" at a day's top right: how much of what is due that day is
/// done. Hover for the counts; click for the dropdown. Green when all is
/// done, red when the day has passed with some still open.
fn completion_badge<'a>(day: Date, done: DueDone, today: Date) -> Element<'a, Message> {
    let tone = if done.done == done.due {
        Tone::Done
    } else if day < today {
        Tone::Late
    } else {
        Tone::Open
    };
    let badge = button(
        text(format!("{}%", done.percent()))
            .size(BADGE_SIZE)
            .line_height(LINE)
            .wrapping(Wrapping::None),
    )
    .padding([0.0, 5.0])
    .style(badge_style(tone))
    .on_press(Message::ToggleDue(day));
    tooltip(
        badge,
        theme::tip(done.describe()),
        tooltip::Position::Bottom,
    )
    .into()
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tone {
    Open,
    Done,
    Late,
}

fn badge_style(tone: Tone) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let p = theme.palette();
        let color = match tone {
            Tone::Open => theme::muted_color(theme),
            Tone::Done => p.success,
            Tone::Late => theme::danger_color(theme),
        };
        let amount = match status {
            button::Status::Hovered | button::Status::Pressed => 0.22,
            _ => 0.12,
        };
        button::Style {
            background: Some(color.scale_alpha(amount).into()),
            text_color: color,
            border: border::rounded(999),
            ..button::Style::default()
        }
    }
}

/// Rough height of the dropdown for `n` tasks, to keep it on screen.
fn dropdown_height(n: usize) -> f32 {
    let card = TITLE_SIZE * LINE + PILL_PAD_Y * 2.0 + 4.0;
    20.0 + DATE_SIZE * LINE + 8.0 + n as f32 * (card + 4.0)
}

/// The dropdown listing the tasks due on a day: open ones first, done ones
/// at the bottom.
fn due_dropdown<'a>(due: Vec<&Task>, today: Date) -> Element<'a, Message> {
    let done = due.iter().filter(|t| t.status == TaskStatus::Done).count();
    let heading = text(format!("{done} of {} done", due.len()))
        .size(TITLE_SIZE)
        .style(theme::muted);
    let mut list = Column::new().spacing(4).push(heading);
    for t in due {
        let line = TitleLine {
            title: t.title.clone(),
            done: t.status == TaskStatus::Done,
            overdue: t.is_overdue(today),
            priority: t.priority,
        };
        list = list.push(task_pill(
            &line,
            DROPDOWN_WIDTH - 20.0 - PILL_PAD_X * 2.0 + TITLE_OVERHANG,
        ));
    }
    container(list.width(Fill))
        .padding(10)
        .width(DROPDOWN_WIDTH)
        .style(theme::overlay_panel)
        .into()
}

/// [`theme::calendar_cell`] as a button style, a little darker on hover.
fn cell_button(cell: theme::Cell) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let base = theme::calendar_cell(cell)(theme);
        let p = theme.palette();
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let background = match (base.background, hovered) {
            (Some(iced::Background::Color(c)), true) => Some(theme::mix(c, p.text, 0.05).into()),
            (None, true) => Some(p.text.scale_alpha(0.04).into()),
            (bg, false) => bg,
            (bg, true) => bg,
        };
        button::Style {
            background,
            text_color: base.text_color.unwrap_or(p.text),
            border: Border {
                radius: (theme::RADIUS - 2.0).into(),
                ..base.border
            },
            ..button::Style::default()
        }
    }
}

// ---------------------------------------------------------- pure helpers

/// One task title as a cell shows it.
#[derive(Debug, Clone, PartialEq)]
struct TitleLine {
    title: String,
    /// Done: dimmed.
    done: bool,
    /// Open with a deadline before today: red.
    overdue: bool,
    /// Shown as `!` / `!!` on open tasks.
    priority: u8,
}

/// What a day cell lists.
#[derive(Debug, Clone, PartialEq, Default)]
struct CellContent {
    titles: Vec<TitleLine>,
    /// How many tasks on that day are not listed ("+N more").
    more: usize,
}

/// The tasks on `day` (by `Task::day()`) as a cell lists them, given room
/// for `lines` text lines under the date (a "+N more" line counts as one).
/// Open tasks first (higher priority, then manual order), then done ones;
/// at most [`MAX_TITLES`] titles. Deleted (`Dropped`) tasks are left out.
fn cell_content(tasks: &[Task], day: Date, today: Date, lines: usize) -> CellContent {
    let mut on_day: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.day() == Some(day) && t.status != TaskStatus::Dropped)
        .collect();
    on_day.sort_by(|a, b| {
        let open = |t: &Task| !t.status.is_open();
        open(a)
            .cmp(&open(b))
            .then(b.priority.cmp(&a.priority))
            .then(a.position.total_cmp(&b.position))
            .then(a.id.cmp(&b.id))
    });
    let total = on_day.len();
    let cap = lines.min(MAX_TITLES);
    let shown = if total <= cap {
        total
    } else {
        // Leave a line for "+N more" when the lines are the limit.
        MAX_TITLES.min(lines.saturating_sub(1))
    };
    CellContent {
        titles: on_day
            .iter()
            .take(shown)
            .map(|t| TitleLine {
                title: t.title.clone(),
                done: !t.status.is_open(),
                overdue: t.is_overdue(today),
                priority: t.priority,
            })
            .collect(),
        more: total - shown,
    }
}

/// How many tasks are due on a day and how many of those are done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DueDone {
    due: usize,
    done: usize,
}

impl DueDone {
    /// Rounded down, so "100%" only when all are done.
    fn percent(self) -> usize {
        self.done * 100 / self.due.max(1)
    }

    /// The badge's tooltip: "3 due · 2 complete".
    fn describe(self) -> String {
        format!("{} due · {} complete", self.due, self.done)
    }
}

/// The tasks due on `day` (deleted ones left out): open ones first (higher
/// priority, then manual order), done ones at the bottom.
fn due_tasks(tasks: &[Task], day: Date) -> Vec<&Task> {
    let mut due: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.due_on == Some(day) && t.status != TaskStatus::Dropped)
        .collect();
    due.sort_by(|a, b| {
        let done = |t: &Task| t.status == TaskStatus::Done;
        done(a)
            .cmp(&done(b))
            .then(b.priority.cmp(&a.priority))
            .then(a.position.total_cmp(&b.position))
            .then(a.id.cmp(&b.id))
    });
    due
}

/// The counts behind a day's badge; `None` when nothing is due that day.
fn due_done(tasks: &[Task], day: Date) -> Option<DueDone> {
    let due = due_tasks(tasks, day);
    (!due.is_empty()).then(|| DueDone {
        due: due.len(),
        done: due.iter().filter(|t| t.status == TaskStatus::Done).count(),
    })
}

/// The width of `s` as a card title draws it (Inter at [`TITLE_SIZE`]),
/// measured with the same text engine the app renders with.
fn measure(s: &str) -> f32 {
    use iced::advanced::text::{self as adv, Paragraph as _};
    type Para = <iced::Renderer as adv::Renderer>::Paragraph;
    Para::with_text(adv::Text {
        content: s,
        bounds: Size::INFINITE,
        size: TITLE_SIZE.into(),
        line_height: LINE.into(),
        font: theme::FONT,
        align_x: adv::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: adv::Shaping::Advanced,
        wrapping: adv::Wrapping::None,
    })
    .min_width()
}

/// [`fit`] with [`measure`], remembered per title and width: the grid is
/// rebuilt often and shaping text is not free.
fn fit_cached(s: &str, max_width: f32) -> String {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static CACHE: RefCell<HashMap<(String, i32), String>> = RefCell::new(HashMap::new());
    }
    let key = (s.to_string(), max_width.floor() as i32);
    CACHE.with_borrow_mut(|cache| {
        if cache.len() > 4096 {
            cache.clear();
        }
        cache
            .entry(key)
            .or_insert_with(|| fit(s, max_width, measure))
            .clone()
    })
}

/// `s` as it fits in `max_width`: whole when it does, otherwise the
/// longest start that fits with "…" after it (at least "…").
fn fit(s: &str, max_width: f32, width: impl Fn(&str) -> f32) -> String {
    if width(s) <= max_width {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let cut = |n: usize| {
        let mut out: String = chars[..n].iter().collect();
        out.truncate(out.trim_end().len());
        out.push('…');
        out
    };
    // The most characters kept such that the cut text fits.
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if width(&cut(mid)) <= max_width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    cut(lo)
}

/// The seven weekdays in grid order, starting at `start`.
fn weekday_order(start: Weekday) -> [Weekday; 7] {
    let mut days = [start; 7];
    for (i, d) in days.iter_mut().enumerate() {
        *d = start.wrapping_add(i as i64);
    }
    days
}

fn weekday_abbr(day: Weekday) -> &'static str {
    match day {
        Weekday::Monday => "Mon",
        Weekday::Tuesday => "Tue",
        Weekday::Wednesday => "Wed",
        Weekday::Thursday => "Thu",
        Weekday::Friday => "Fri",
        Weekday::Saturday => "Sat",
        Weekday::Sunday => "Sun",
    }
}

fn month_abbr(d: Date) -> &'static str {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS[(d.month() - 1) as usize]
}

/// The week number shown on the label: the ISO week of `start + 3 days`,
/// the same rule as `week::week_number`.
fn week_number(week: WeekRef) -> i8 {
    week.start.saturating_add(3.days()).iso_week_date().week()
}

/// Whether the week has a ready summary (the dot).
fn has_summary(data: &MonthData, week: WeekRef) -> bool {
    data.summary_weeks.contains(&week.start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    const TODAY: Date = date(2026, 11, 16);

    fn task(id: i64, title: &str, day: Date, status: TaskStatus) -> Task {
        Task {
            id,
            title: title.into(),
            status,
            planned_for: Some(day),
            position: id as f64,
            ..Task::default()
        }
    }

    fn data() -> MonthData {
        MonthData {
            month: date(2026, 11, 1),
            today: TODAY,
            week_start: Weekday::Monday,
            working_days: vec![
                Weekday::Monday,
                Weekday::Tuesday,
                Weekday::Wednesday,
                Weekday::Thursday,
                Weekday::Friday,
            ],
            tasks: Vec::new(),
            summary_weeks: vec![date(2026, 11, 9)],
        }
    }

    fn titles(c: &CellContent) -> Vec<&str> {
        c.titles.iter().map(|t| t.title.as_str()).collect()
    }

    #[test]
    fn cell_lists_up_to_three_titles_then_a_count() {
        let d = date(2026, 11, 18);
        let tasks: Vec<Task> = (1..=5)
            .map(|i| task(i, &format!("t{i}"), d, TaskStatus::Todo))
            .collect();
        let c = cell_content(&tasks, d, TODAY, 4);
        assert_eq!(titles(&c), ["t1", "t2", "t3"]);
        assert_eq!(c.more, 2);

        // Exactly three: no count.
        let c = cell_content(&tasks[..3], d, TODAY, 4);
        assert_eq!(c.titles.len(), 3);
        assert_eq!(c.more, 0);

        // Four tasks: three titles and "+1 more".
        let c = cell_content(&tasks[..4], d, TODAY, 4);
        assert_eq!(c.titles.len(), 3);
        assert_eq!(c.more, 1);
    }

    #[test]
    fn short_cells_show_fewer_titles() {
        let d = date(2026, 11, 18);
        let tasks: Vec<Task> = (1..=5)
            .map(|i| task(i, &format!("t{i}"), d, TaskStatus::Todo))
            .collect();
        // Room for two lines: one title and "+4 more".
        let c = cell_content(&tasks, d, TODAY, 2);
        assert_eq!(titles(&c), ["t1"]);
        assert_eq!(c.more, 4);
        // Two tasks fit two lines exactly: no count.
        let c = cell_content(&tasks[..2], d, TODAY, 2);
        assert_eq!(c.titles.len(), 2);
        assert_eq!(c.more, 0);
    }

    #[test]
    fn open_tasks_come_first_and_flags_are_set() {
        let d = date(2026, 11, 18);
        let mut overdue = task(3, "late", d, TaskStatus::Doing);
        overdue.due_on = Some(date(2026, 11, 10));
        let mut urgent = task(4, "urgent", d, TaskStatus::Todo);
        urgent.priority = 2;
        let tasks = vec![
            task(1, "done", d, TaskStatus::Done),
            task(2, "dropped", d, TaskStatus::Dropped),
            overdue,
            urgent,
            task(5, "other day", date(2026, 11, 19), TaskStatus::Todo),
        ];
        let c = cell_content(&tasks, d, TODAY, 4);
        // The dropped (deleted) task is not listed or counted.
        assert_eq!(titles(&c), ["urgent", "late", "done"]);
        assert_eq!(c.more, 0);
        assert!(!c.titles[0].done && !c.titles[0].overdue);
        assert!(c.titles[1].overdue && !c.titles[1].done);
        assert!(c.titles[2].done && !c.titles[2].overdue);
    }

    #[test]
    fn tasks_are_placed_by_planned_day_not_due_date() {
        let mut planned = task(1, "planned", date(2026, 11, 18), TaskStatus::Todo);
        planned.due_on = Some(date(2026, 11, 20));
        let mut due = task(2, "due", date(2026, 11, 20), TaskStatus::Todo);
        due.planned_for = None;
        due.due_on = Some(date(2026, 11, 20));
        let tasks = vec![planned, due];
        let c = cell_content(&tasks, date(2026, 11, 20), TODAY, 4);
        assert!(titles(&c).is_empty());
        let c = cell_content(&tasks, date(2026, 11, 18), TODAY, 4);
        assert_eq!(titles(&c), ["planned"]);
        assert_eq!(
            cell_content(&tasks, TODAY, TODAY, 4),
            CellContent::default()
        );
    }

    #[test]
    fn weekday_header_follows_week_start() {
        use Weekday::*;
        assert_eq!(
            weekday_order(Monday),
            [
                Monday, Tuesday, Wednesday, Thursday, Friday, Saturday, Sunday
            ]
        );
        assert_eq!(
            weekday_order(Sunday),
            [
                Sunday, Monday, Tuesday, Wednesday, Thursday, Friday, Saturday
            ]
        );
        let names: Vec<_> = weekday_order(Sunday).map(weekday_abbr).to_vec();
        assert_eq!(names, ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]);
    }

    #[test]
    fn summary_dot_and_week_number() {
        let d = data();
        assert!(has_summary(&d, WeekRef::new(date(2026, 11, 9))));
        assert!(!has_summary(&d, WeekRef::new(date(2026, 11, 16))));
        assert_eq!(week_number(WeekRef::new(date(2026, 11, 16))), 47);
        // A Sunday-start week uses the same rule (Sun 15 + 3 = Wed 18).
        assert_eq!(week_number(WeekRef::new(date(2026, 11, 15))), 47);
    }

    #[test]
    fn titles_truncate_with_an_ellipsis() {
        // Every character 10 px wide.
        let w = |s: &str| s.chars().count() as f32 * 10.0;
        assert_eq!(fit("short", 50.0, w), "short");
        assert_eq!(fit("exactly10!", 100.0, w), "exactly10!");
        assert_eq!(fit("Write the Q4 report", 100.0, w), "Write the…");
        assert_eq!(fit("Write the Q4 report", 5.0, w), "…");
        // Narrow letters fit more: width decides, not the character count.
        let narrow = |s: &str| s.chars().map(|c| if c == 'i' { 3.0 } else { 10.0 }).sum();
        assert_eq!(fit("iiiiiiiiiiiiiiii", 60.0, narrow), "iiiiiiiiiiiiiiii");
    }

    #[test]
    fn row_height_decides_how_many_lines_fit() {
        assert_eq!(lines_that_fit(cell_height(4)), 4);
        assert_eq!(lines_that_fit(cell_height(2) + 1.0), 2);
        assert_eq!(lines_that_fit(cell_height(0) - 5.0), 0);
    }

    #[test]
    fn due_tasks_count_done_and_sort_done_last() {
        let d = date(2026, 11, 18);
        let due = |mut t: Task| {
            t.due_on = Some(d);
            t
        };
        let tasks = vec![
            due(task(1, "done", d, TaskStatus::Done)),
            due(task(2, "open", date(2026, 11, 17), TaskStatus::Todo)),
            due(task(3, "deleted", d, TaskStatus::Dropped)),
            // On the 18th but due another day: not counted.
            task(4, "planned only", d, TaskStatus::Todo),
            due(task(5, "doing", d, TaskStatus::Doing)),
        ];
        let ids: Vec<i64> = due_tasks(&tasks, d).iter().map(|t| t.id).collect();
        assert_eq!(ids, [2, 5, 1]);
        let counts = due_done(&tasks, d).unwrap();
        assert_eq!(counts, DueDone { due: 3, done: 1 });
        assert_eq!(counts.percent(), 33);
        assert_eq!(counts.describe(), "3 due · 1 complete");
        assert_eq!(due_done(&tasks, date(2026, 11, 19)), None);
        assert_eq!(DueDone { due: 3, done: 3 }.percent(), 100);
        assert_eq!(DueDone { due: 3, done: 2 }.percent(), 66);
    }

    #[test]
    fn the_badge_toggles_its_dropdown() {
        let mut v = MonthView::new();
        v.load(data());
        let d = date(2026, 11, 18);
        assert_eq!(v.update(Message::ToggleDue(d)), Action::None);
        assert_eq!(v.open_due, Some(d));
        v.update(Message::Ignore);
        assert_eq!(v.open_due, Some(d));
        v.update(Message::ToggleDue(d));
        assert_eq!(v.open_due, None);
        v.update(Message::ToggleDue(d));
        v.update(Message::CloseDue);
        assert_eq!(v.open_due, None);
        let _ = grid(&data(), Some(d), Size::new(1000.0, 700.0));
    }

    #[test]
    fn clicks_map_to_actions() {
        let mut v = MonthView::new();
        let d = date(2026, 11, 17);
        assert_eq!(v.update(Message::DayClicked(d)), Action::OpenDay(d));
        let w = WeekRef::new(date(2026, 11, 16));
        assert_eq!(v.update(Message::WeekClicked(w)), Action::OpenWeek(w));
    }

    #[test]
    fn load_keeps_data_and_ignores_repeats() {
        let mut v = MonthView::new();
        assert!(v.data.is_none());
        v.load(data());
        assert_eq!(v.data, Some(data()));
        // Same data again: unchanged.
        v.load(data());
        assert_eq!(v.data, Some(data()));
        // New data replaces it.
        let mut next = data();
        next.month = date(2026, 12, 1);
        v.load(next.clone());
        assert_eq!(v.data, Some(next));
    }
}
