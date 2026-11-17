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
//! The grid fills the content area with equal cells. When the rows get too
//! short for three titles it shows fewer (the "+N more" grows); below one
//! title per cell it keeps a minimum height and scrolls.
//!
//! Skipped (prototype scope): keyboard navigation inside the grid, hover
//! tooltips with full titles, and pixel-exact truncation (titles are cut to
//! an estimated character count with "…" and clipped at the cell edge).

use crate::theme;
use fortnight_core::domain::{Task, TaskStatus, WeekRef};
use fortnight_core::week::month_weeks;
use iced::border::{self, Border};
use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, Space, button, column, container, responsive, row, scrollable, text,
};
use iced::{Color, Element, Fill, Font, Length, Size, Theme};
use jiff::ToSpan;
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
        responsive(move |size| grid(data, size)).into()
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
/// Average glyph width of Inter as a fraction of the font size; slightly
/// generous so titles are cut before they reach the clip edge.
const GLYPH_WIDTH: f32 = 0.54;

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

/// The grid for `size` (the content area below the top bar).
fn grid(data: &MonthData, size: Size) -> Element<'_, Message> {
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
    let max_chars = ((cell_width - CELL_PAD_X * 2.0 - PILL_PAD_X * 2.0)
        / (TITLE_SIZE * GLYPH_WIDTH))
        .floor()
        .max(1.0) as usize;

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
    for week in weeks {
        let mut line = Row::new()
            .spacing(GAP)
            .height(row_height)
            .push(week_label(week, has_summary(data, week)));
        for day in week.days() {
            line = line.push(day_cell(data, day, lines, max_chars));
        }
        body = body.push(line);
    }

    let body: Element<'_, Message> = if scrolls {
        scrollable(body).height(Fill).into()
    } else {
        body.into()
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
                background: Some(theme::ACCENT.into()),
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
fn day_cell(data: &MonthData, day: Date, lines: usize, max_chars: usize) -> Element<'_, Message> {
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

    let mut cards = Column::new().spacing(LINE_GAP);
    for t in &content.titles {
        cards = cards.push(task_pill(t, max_chars));
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
    let col = column![date, cards].spacing(DATE_GAP);

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

/// One task as a small card, like the Week tab's: the priority `!` in its
/// colour, then the title. Overdue is red on a red tint; done or dropped is
/// faint on a fainter fill.
fn task_pill<'a>(t: &TitleLine, max_chars: usize) -> Element<'a, Message> {
    let (done, overdue) = (t.done, t.overdue);
    let mut line = Row::new().spacing(3);
    let level = t.priority.min(2);
    let mut chars = max_chars;
    if !done && level > 0 {
        chars = chars.saturating_sub(usize::from(level) + 1);
        line = line.push(
            text("!".repeat(usize::from(level)))
                .size(TITLE_SIZE)
                .line_height(LINE)
                .style(move |theme: &Theme| text::Style {
                    color: theme::priority_color(theme, level),
                }),
        );
    }
    line = line.push(
        text(truncate(&t.title, chars.max(1)))
            .size(TITLE_SIZE)
            .line_height(LINE)
            .wrapping(Wrapping::None)
            .style(move |theme: &Theme| text::Style {
                color: Some(if overdue {
                    theme::danger_color(theme)
                } else if done {
                    theme.palette().text.scale_alpha(0.4)
                } else {
                    theme.palette().text
                }),
            }),
    );
    container(line)
        .padding([PILL_PAD_Y, PILL_PAD_X])
        .width(Fill)
        .clip(true)
        .style(move |theme: &Theme| {
            let p = theme.palette();
            let fill = if overdue {
                theme::danger_color(theme).scale_alpha(0.14)
            } else {
                mix(p.background, p.text, if done { 0.06 } else { 0.11 })
            };
            container::Style {
                background: Some(fill.into()),
                border: border::rounded(5),
                ..container::Style::default()
            }
        })
        .into()
}

/// [`theme::calendar_cell`] as a button style, a little darker on hover.
fn cell_button(cell: theme::Cell) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let base = theme::calendar_cell(cell)(theme);
        let p = theme.palette();
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let background = match (base.background, hovered) {
            (Some(iced::Background::Color(c)), true) => Some(mix(c, p.text, 0.05).into()),
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

/// `a` moved `amount` (0–1) towards `b`.
fn mix(a: Color, b: Color, amount: f32) -> Color {
    Color::from_rgba(
        a.r + (b.r - a.r) * amount,
        a.g + (b.g - a.g) * amount,
        a.b + (b.b - a.b) * amount,
        a.a,
    )
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

/// `s` cut to at most `max` characters, ending in "…" when cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.truncate(out.trim_end().len());
    out.push('…');
    out
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
/// the same rule as `week::week_label`.
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
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exactly10!", 10), "exactly10!");
        assert_eq!(truncate("Write the Q4 report", 10), "Write the…");
        assert_eq!(truncate("Write the Q4 report", 1), "…");
    }

    #[test]
    fn row_height_decides_how_many_lines_fit() {
        assert_eq!(lines_that_fit(cell_height(4)), 4);
        assert_eq!(lines_that_fit(cell_height(2) + 1.0), 2);
        assert_eq!(lines_that_fit(cell_height(0) - 5.0), 0);
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
