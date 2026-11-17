//! The Today tab: one day's tasks (PLAN.md 4.2 "Today").
//!
//! Sections, top to bottom:
//!
//! - **Recommended** (only when the shown day is today): the model's top
//!   three open tasks (on the Tasks list, carried-over ones included, or
//!   overdue), most important
//!   first, numbered (the model's reasons aren't shown). They are left out of
//!   the sections below.
//! - **Tasks**: open tasks whose [`Task::day`] (the planned day) is the
//!   shown day, then the add-a-task input. On today this also carries over
//!   open tasks planned for an earlier day whose deadline has not passed.
//! - **Overdue** (only when the shown day is today, and only if any): open
//!   tasks whose deadline has passed.
//! - **Today's Notes** (today): a free-text box to jot down anything from
//!   the day. Saved a moment after typing stops, as one free-standing log
//!   line dated today (replacing that day's others), so it reaches the
//!   weekly summary as `log_lines`. Older side work lines show up as lines
//!   of the notes. Other days show their notes read only, if any.
//! - **Completed**, collapsed: done tasks whose day is the shown day (as on
//!   the Week tab), whenever they were ticked; a done task with no day shows
//!   on the day it was ticked.
//! - **No date**, collapsed: open tasks with no planned day (the inbox).
//! - **Deleted**, collapsed: every deleted task, newest first, each a quiet
//!   card with Restore and Delete forever, and Delete all in the header
//!   when open. Delete moves a task here (status `Dropped`); only Delete
//!   forever (or Delete all) removes it.
//!
//! Section titles show the count only (no estimate totals).
//!
//! The quick-add bar and the date navigation live in the top bar.
//!
//! Clicking a row (or `Enter` on the selected row) expands it in place into
//! an edit form, one row at a time: title, project (with a list of the
//! projects of open tasks), due date (a calendar; only the deadline, it
//! does not move the task: the planned day is set by quick add or by
//! dragging in the Week tab),
//! estimate, priority (Low, Medium, High), subtasks and notes. Subtasks are
//! kept in the task's notes as Markdown checklist lines (`- [ ] text`), so
//! they need no schema change and reach the summary with the notes.
//! `↑` `↓` move a selection highlight through the visible rows (stopping at
//! the ends), `Space` toggles done and `Escape` collapses the form, then
//! clears the selection. Closing a form with the mouse (Save, Close, or a
//! click on its row) clears the selection too, so no accent ring is left
//! behind.
//!
//! The statuses `Doing` (in progress) are shown as plain open tasks; the app
//! no longer sets it.
//!
//! The Tasks section is in manual order (`Task::position`) and can be
//! rearranged by dragging a card: a copy follows the pointer and an accent
//! line shows where it will land; letting go renumbers the section (1, 2,
//! 3, …). A press let go without moving is a click (it opens the card).
//! The other sections keep their own order and don't drag.
//!
//! Skipped for the prototype: undo for a drag, strikethrough on done tasks
//! (iced text has none), editing the notes of a day other than today (the
//! store stamps log lines now), undo for the notes, and saving notes typed
//! in the last moment before quitting.

use std::cmp::Ordering;
use std::time::{Duration, Instant};

use fortnight_core::domain::{EventId, NewTask, Priority, Task, TaskId, TaskStatus};
use fortnight_core::parse;
use fortnight_core::week::add_months;
use iced::widget::overlay::menu;
use iced::widget::{
    Column, Row, Space, button, checkbox, column, combo_box, container, mouse_area, pin, row,
    scrollable, stack, text, text_editor, text_input,
};
use iced::{Center, Color, Element, Fill, Point, Theme, border, mouse};
use jiff::Timestamp;
use jiff::civil::{Date, Weekday};
use jiff::tz::TimeZone;

use super::NavKey;
use crate::calendar::{Popup, month_picker};
use crate::theme;

#[derive(Debug, Clone)]
pub enum Message {
    /// The checkbox on a row.
    Toggle(TaskId, bool),
    /// A click on a row: expand it (or collapse it if open) and select it.
    Expand(TaskId),
    EditTitle(String),
    EditProject(String),
    /// A project picked from the list.
    PickProject(String),
    EditEstimate(String),
    EditNotes(text_editor::Action),
    SetPriority(Priority),
    /// Open or close the due-date calendar.
    ToggleCalendar,
    /// A click outside the open calendar.
    CloseCalendar,
    /// Show the calendar month before (-1) or after (+1).
    CalendarMonth(i32),
    PickDue(Date),
    ClearDue,
    ToggleSubtask(usize, bool),
    RemoveSubtask(usize),
    EditNewSubtask(String),
    AddSubtask,
    /// The add-a-task input under the day's tasks.
    EditQuickAdd(String),
    SubmitQuickAdd,
    /// Typing in Today's Notes. Saved by `main.rs` once typing pauses
    /// ([`DayView::notes_to_save`]).
    EditDayNotes(text_editor::Action),
    Save,
    /// Move the open task to Deleted.
    Delete,
    Close,
    /// Put a deleted task back.
    Restore(TaskId),
    /// The × on a completed task: move it to Deleted.
    Remove(TaskId),
    /// Remove a deleted task for good.
    DeleteForever(TaskId),
    /// Remove every deleted task for good. `main.rs` handles it (one undo
    /// for all); `update` ignores it.
    DeleteAll,
    ToggleShowDone,
    ToggleShowNoDate,
    ToggleShowDeleted,
    /// The mouse went down on a Tasks card.
    Press(TaskId),
    /// The pointer moved while a card is held (position in the tab).
    Moved(Point),
    /// The pointer entered a card (or, while dragging, the space after the
    /// last card).
    Enter(Spot),
    /// The pointer left a card.
    Leave(TaskId),
    /// A dragged card was let go there.
    DropOn(Spot),
    /// The mouse button went up anywhere: the drag (if any) is over.
    EndDrag,
}

impl Message {
    /// Pointer bookkeeping that changes no data: `main.rs` skips its
    /// refresh for these, as they arrive with every mouse move.
    pub fn is_pointer_move(&self) -> bool {
        matches!(
            self,
            Message::Press(_)
                | Message::Moved(_)
                | Message::Enter(_)
                | Message::Leave(_)
                | Message::EndDrag
        )
    }
}

/// Where in the Tasks section a dragged card would land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    /// Before this task's card.
    Before(TaskId),
    /// After the last card.
    End,
}

/// How far the pointer must move before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 4.0;

/// A held Tasks card.
#[derive(Debug, Clone, PartialEq)]
struct Drag {
    id: TaskId,
    /// Where the pointer was first seen after the press.
    start: Option<Point>,
    /// Where it is now.
    at: Option<Point>,
    /// Moved past `DRAG_THRESHOLD`: a drag, not a click.
    active: bool,
    over: Option<Spot>,
}

/// What `main.rs` must do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Set a task's status: the checkbox (`Done`/`Todo`), Delete
    /// (`Dropped`) and Restore (`Todo`).
    SetStatus(TaskId, TaskStatus),
    /// Save the editable fields. `main.rs` records it for undo.
    Update(Task),
    /// Remove a task for good (Delete forever).
    Delete(TaskId),
    /// Add a log line: attached to a task, or free-standing (`None`).
    /// `main.rs` stamps it with the current time.
    AddLog {
        task_id: Option<TaskId>,
        text: String,
    },
    DeleteLog(EventId),
    /// Create a task (the add-a-task input). `main.rs` records it for undo.
    Add(NewTask),
    /// The Tasks section was rearranged: set each task's position.
    Reorder(Vec<(TaskId, f64)>),
}

/// A task the model recommends doing today.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recommended {
    pub id: TaskId,
    /// A few words on why.
    pub why: String,
}

/// A log line, as the view needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub id: EventId,
    /// `None` for a free-standing log line.
    pub task_id: Option<TaskId>,
    pub at: Timestamp,
    /// The local date of `at`.
    pub date: Date,
    pub text: String,
}

/// Everything the view shows, supplied by `main.rs`.
#[derive(Debug, Clone, PartialEq)]
pub struct DayData {
    /// The day shown (the shared selected date).
    pub day: Date,
    /// The local date today. Overdue is relative to this.
    pub today: Date,
    /// For the local date a task was completed on.
    pub tz: TimeZone,
    /// Every task, in any status.
    pub tasks: Vec<Task>,
    /// The first day of the week (Settings), for the due-date calendar.
    pub week_start: Weekday,
    /// The model's recommended tasks for today, most important first (from
    /// this week's Recommended tasks report). Shown only on today.
    pub recommended: Vec<Recommended>,
    /// Recommendations are being worked out.
    pub recommending: bool,
    /// Every log line, oldest first. Only free-standing ones are shown.
    pub logs: Vec<LogLine>,
}

#[derive(Debug, Default)]
pub struct DayView {
    data: Option<DayData>,
    show_done: bool,
    show_no_date: bool,
    show_deleted: bool,
    /// The keyboard selection highlight.
    selected: Option<TaskId>,
    /// The expanded row's form.
    editing: Option<EditForm>,
    /// Today's Notes: always today's, whatever day is shown.
    notes: text_editor::Content,
    /// When the notes were last typed in, while not yet saved.
    notes_edited: Option<Instant>,
    /// The add-a-task input.
    quick_add: String,
    /// A held Tasks card.
    drag: Option<Drag>,
    /// The Tasks card under the pointer.
    hovered: Option<TaskId>,
}

/// Where a task is shown on the Today tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    /// Today only: the model's top three open tasks, most important first.
    /// They are left out of the sections below.
    Recommended,
    /// The shown day's open tasks.
    Day,
    Overdue,
    Done,
    NoDate,
    Deleted,
}

impl Section {
    /// Display order of the task sections (side work sits between Overdue
    /// and Done).
    pub const ALL: [Section; 6] = [
        Section::Recommended,
        Section::Day,
        Section::Overdue,
        Section::Done,
        Section::NoDate,
        Section::Deleted,
    ];
}

/// The local date a done task was completed on; its day if the completion
/// time is missing.
fn completed_on(task: &Task, tz: &TimeZone) -> Option<Date> {
    match task.completed_at {
        Some(at) => Some(tz.to_datetime(at).date()),
        None => task.day(),
    }
}

/// The section `task` belongs in when `day` is shown, or `None` if it is not
/// on screen. See the module doc for the rules.
pub fn section_of(task: &Task, day: Date, today: Date, tz: &TimeZone) -> Option<Section> {
    match task.status {
        TaskStatus::Dropped => return Some(Section::Deleted),
        // Counted as done on the day it sits on (as on the Week tab), not
        // the day the box was ticked; that only for a task with no day.
        TaskStatus::Done => {
            let done_on = task.day().or_else(|| completed_on(task, tz));
            return (done_on == Some(day)).then_some(Section::Done);
        }
        TaskStatus::Todo | TaskStatus::Doing => {}
    }
    let shows_today = day == today;
    let Some(task_day) = task.day() else {
        return Some(Section::NoDate);
    };
    if shows_today && task.is_overdue(today) {
        Some(Section::Overdue)
    } else if task_day == day || (shows_today && task_day < today) {
        Some(Section::Day)
    } else {
        None
    }
}

/// Order for open tasks: higher priority, then earlier day (carried-over
/// before today's), then manual position, then id.
fn open_order(a: &Task, b: &Task) -> Ordering {
    b.priority
        .cmp(&a.priority)
        .then(a.day().cmp(&b.day()))
        .then(a.position.total_cmp(&b.position))
        .then(a.id.cmp(&b.id))
}

/// How many recommended tasks the Today tab shows.
pub const MAX_RECOMMENDED: usize = 3;

/// The recommended tasks shown: only when the shown day is today, those
/// still open, at most [`MAX_RECOMMENDED`], in the model's order.
fn recommended_tasks(data: &DayData) -> Vec<&Task> {
    if data.day != data.today {
        return Vec::new();
    }
    data.recommended
        .iter()
        .filter_map(|r| data.tasks.iter().find(|t| t.id == r.id))
        .filter(|t| t.status.is_open())
        .take(MAX_RECOMMENDED)
        .collect()
}

/// The section `task` is shown in: Recommended if it is one of
/// [`recommended_tasks`], otherwise [`section_of`].
fn placed_section(data: &DayData, task: &Task) -> Option<Section> {
    if recommended_tasks(data).iter().any(|t| t.id == task.id) {
        Some(Section::Recommended)
    } else {
        section_of(task, data.day, data.today, &data.tz)
    }
}

/// The tasks in `section`, sorted. Recommended: the model's order.
/// Overdue: oldest deadline first, then [`open_order`]. The day: manual
/// order (`position`). No date: [`open_order`]. Completed: most recently finished first. Deleted:
/// most recently deleted first.
pub fn section_tasks(data: &DayData, section: Section) -> Vec<&Task> {
    if section == Section::Recommended {
        return recommended_tasks(data);
    }
    let mut out: Vec<&Task> = data
        .tasks
        .iter()
        .filter(|t| placed_section(data, t) == Some(section))
        .collect();
    match section {
        Section::Recommended => {}
        Section::Overdue => out.sort_by(|a, b| a.due_on.cmp(&b.due_on).then(open_order(a, b))),
        // The day's tasks are in manual order (drag to rearrange).
        Section::Day => out.sort_by(|a, b| a.position.total_cmp(&b.position).then(a.id.cmp(&b.id))),
        Section::NoDate => out.sort_by(|a, b| open_order(a, b)),
        Section::Done | Section::Deleted => out.sort_by(|a, b| {
            let when = |t: &Task| t.completed_at.unwrap_or(t.updated_at);
            when(b).cmp(&when(a)).then(a.id.cmp(&b.id))
        }),
    }
    out
}

/// The rows the keyboard moves through, top to bottom, skipping collapsed
/// sections and Deleted.
pub fn visible_rows(data: &DayData, show_done: bool, show_no_date: bool) -> Vec<TaskId> {
    Section::ALL
        .into_iter()
        .filter(|s| match s {
            Section::Done => show_done,
            Section::NoDate => show_no_date,
            Section::Deleted => false,
            _ => true,
        })
        .flat_map(|s| section_tasks(data, s).into_iter().map(|t| t.id))
        .collect()
}

/// Days from `today` to `date` (negative in the past).
fn days_between(today: Date, date: Date) -> i64 {
    today
        .until(date)
        .map(|span| i64::from(span.get_days()))
        .unwrap_or(0)
}

/// A short relative label for a due date:
/// `Yesterday`, `3d ago`, `Today`, `Tomorrow`, `Fri` (2–6 days),
/// `in 9d` (7–13 days), then `Nov 20`, with the year if it differs.
pub fn due_label(due: Date, today: Date) -> String {
    match days_between(today, due) {
        -1 => "Yesterday".to_string(),
        d if d < -1 => format!("{}d ago", -d),
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        2..=6 => due.strftime("%a").to_string(),
        d @ 7..=13 => format!("in {d}d"),
        _ if due.year() == today.year() => due.strftime("%b %-d").to_string(),
        _ => due.strftime("%b %-d, %Y").to_string(),
    }
}

/// The status the checkbox or `Space` moves a task to.
fn toggled(status: TaskStatus) -> TaskStatus {
    if status.is_open() {
        TaskStatus::Done
    } else {
        TaskStatus::Todo
    }
}

// ---------------------------------------------------------------- priority

/// The three priorities the app offers, and the value each stores. Old
/// data's 3 (`!!!`) shows as High.
pub const PRIORITIES: [(&str, Priority); 3] = [("Low", 0), ("Medium", 1), ("High", 2)];

/// The stored priority as Low (0), Medium (1) or High (2).
fn priority_level(p: Priority) -> Priority {
    p.min(2)
}

// ---------------------------------------------------------------- subtasks

/// A checklist item kept in the notes as `- [ ] text` or `- [x] text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subtask {
    pub done: bool,
    pub text: String,
}

/// One notes line as a subtask, if it is a checklist line.
fn subtask_line(line: &str) -> Option<Subtask> {
    let rest = line.trim_start().strip_prefix("- [")?;
    let (mark, text) = rest.split_at_checked(1)?;
    let text = text.strip_prefix("] ")?;
    let done = match mark {
        " " => false,
        "x" | "X" => true,
        _ => return None,
    };
    Some(Subtask {
        done,
        text: text.trim().to_string(),
    })
}

/// Splits notes into their subtasks and the remaining text.
pub fn split_notes(notes: &str) -> (Vec<Subtask>, String) {
    let mut subtasks = Vec::new();
    let mut rest = Vec::new();
    for line in notes.lines() {
        match subtask_line(line) {
            Some(s) => subtasks.push(s),
            None => rest.push(line),
        }
    }
    (subtasks, rest.join("\n").trim().to_string())
}

/// Joins notes text and subtasks back into one notes string: the text, a
/// blank line, then the checklist.
pub fn join_notes(subtasks: &[Subtask], text: &str) -> String {
    let list: Vec<String> = subtasks
        .iter()
        .map(|s| format!("- [{}] {}", if s.done { "x" } else { " " }, s.text))
        .collect();
    let text = text.trim();
    match (text.is_empty(), list.is_empty()) {
        (_, true) => text.to_string(),
        (true, false) => list.join("\n"),
        (false, false) => format!("{text}\n\n{}", list.join("\n")),
    }
}

/// `(done, total)` subtasks in the notes.
fn subtask_counts(notes: &str) -> (usize, usize) {
    notes
        .lines()
        .filter_map(subtask_line)
        .fold((0, 0), |(d, t), s| (d + usize::from(s.done), t + 1))
}

// ------------------------------------------------------------------- form

/// The in-place edit form for one task.
#[derive(Debug, Clone)]
pub struct EditForm {
    pub id: TaskId,
    pub title: String,
    pub project: String,
    /// The projects of open tasks, for the project field's list.
    pub projects: combo_box::State<String>,
    /// The deadline.
    pub due: Option<Date>,
    /// The month the calendar shows, while it is open.
    pub calendar: Option<Date>,
    /// The first day of the calendar's weeks (the Settings week start).
    pub week_start: Weekday,
    pub estimate: String,
    /// The estimate as first shown, so an untouched field is not re-parsed.
    pub original_estimate: String,
    pub priority: Priority,
    /// The priority the decision model suggests for a new task, shown as
    /// a faint outline on that option. Set by `main.rs`.
    pub suggested: Option<Priority>,
    pub subtasks: Vec<Subtask>,
    pub new_subtask: String,
    /// The notes without the subtasks.
    pub notes: text_editor::Content,
    original_split: (Vec<Subtask>, String),
    pub error: Option<String>,
}

impl EditForm {
    pub fn from_task(task: &Task) -> Self {
        let estimate = task
            .estimate_minutes
            .map(parse::format_estimate)
            .unwrap_or_default();
        let (subtasks, text) = split_notes(&task.notes);
        Self {
            id: task.id,
            title: task.title.clone(),
            project: task.project.clone().unwrap_or_default(),
            projects: combo_box::State::default(),
            due: task.due_on,
            calendar: None,
            week_start: Weekday::Monday,
            estimate: estimate.clone(),
            original_estimate: estimate,
            priority: priority_level(task.priority),
            suggested: None,
            subtasks: subtasks.clone(),
            new_subtask: String::new(),
            notes: text_editor::Content::with_text(&text),
            original_split: (subtasks, text),
            error: None,
        }
    }

    /// Builds the updated task from the form, or an error message.
    ///
    /// The due date is only the deadline: the planned day (where the task
    /// is shown) is kept. An unchanged estimate is not re-parsed; a typed
    /// subtask not yet added is added.
    pub fn apply(&self, task: &Task) -> Result<Task, String> {
        let title = self.title.trim();
        if title.is_empty() {
            return Err("The title can't be empty.".to_string());
        }
        let estimate = self.estimate.trim();
        let estimate_minutes = if estimate.is_empty() {
            None
        } else if estimate == self.original_estimate.trim() {
            task.estimate_minutes
        } else {
            Some(parse::parse_estimate(estimate).ok_or_else(|| {
                format!("Can't read \"{estimate}\" as an estimate. Try 30m, 2h or 1h30m.")
            })?)
        };
        let mut subtasks = self.subtasks.clone();
        let typed = self.new_subtask.trim();
        if !typed.is_empty() {
            subtasks.push(Subtask {
                done: false,
                text: typed.to_string(),
            });
        }
        let text = self.notes.text().trim().to_string();
        let notes = if (subtasks.as_slice(), text.as_str())
            == (
                self.original_split.0.as_slice(),
                self.original_split.1.as_str(),
            ) {
            task.notes.clone()
        } else {
            join_notes(&subtasks, &text)
        };
        // Keep an old `!!!` unless the priority was changed.
        let priority = if self.priority == priority_level(task.priority) {
            task.priority
        } else {
            self.priority
        };
        let project = self.project.trim().trim_start_matches('#').trim();
        Ok(Task {
            title: title.to_string(),
            project: (!project.is_empty()).then(|| project.to_string()),
            due_on: self.due,
            estimate_minutes,
            priority,
            notes,
            ..task.clone()
        })
    }
}

/// The distinct projects of open tasks, sorted case-insensitively.
pub fn open_projects(tasks: &[Task]) -> Vec<String> {
    let mut out: Vec<String> = tasks
        .iter()
        .filter(|t| t.status.is_open())
        .filter_map(|t| t.project.clone())
        .collect();
    out.sort_by_key(|p| p.to_lowercase());
    out.dedup();
    out
}

impl EditForm {
    /// An empty form for a new task (the Week tab's dialog), with the
    /// project list from `tasks`.
    pub fn blank(tasks: &[Task], week_start: Weekday) -> Self {
        open_form(&Task::default(), tasks, week_start)
    }

    /// The new task this form describes, planned for `day`, or an error
    /// message.
    pub fn new_task(&self, day: Date) -> Result<NewTask, String> {
        let base = Task {
            planned_for: Some(day),
            ..Task::default()
        };
        let t = self.apply(&base)?;
        Ok(NewTask {
            title: t.title,
            notes: t.notes,
            project: t.project,
            due_on: t.due_on,
            planned_for: t.planned_for,
            estimate_minutes: t.estimate_minutes,
            priority: t.priority,
            private: false,
        })
    }

    /// Applies a message that only edits the form. Returns `false` (and
    /// does nothing) for any other message. Any edit clears the error.
    pub fn handle(&mut self, message: &Message, today: Date) -> bool {
        match message {
            Message::EditTitle(s) => self.title = s.clone(),
            Message::EditProject(s) | Message::PickProject(s) => self.project = s.clone(),
            Message::EditEstimate(s) => self.estimate = s.clone(),
            Message::EditNotes(action) => self.notes.perform(action.clone()),
            Message::SetPriority(p) => self.priority = priority_level(*p),
            Message::ToggleCalendar => {
                self.calendar = match self.calendar {
                    Some(_) => None,
                    None => Some(self.due.unwrap_or(today).first_of_month()),
                }
            }
            Message::CloseCalendar => self.calendar = None,
            Message::CalendarMonth(step) => {
                if let Some(month) = &mut self.calendar {
                    *month = add_months(*month, *step).first_of_month();
                }
            }
            Message::PickDue(date) => {
                self.due = Some(*date);
                self.calendar = None;
            }
            Message::ClearDue => {
                self.due = None;
                self.calendar = None;
            }
            Message::ToggleSubtask(i, done) => {
                if let Some(s) = self.subtasks.get_mut(*i) {
                    s.done = *done;
                }
            }
            Message::RemoveSubtask(i) => {
                if *i < self.subtasks.len() {
                    self.subtasks.remove(*i);
                }
            }
            Message::EditNewSubtask(s) => self.new_subtask = s.clone(),
            Message::AddSubtask => {
                let text = std::mem::take(&mut self.new_subtask).trim().to_string();
                if !text.is_empty() {
                    self.subtasks.push(Subtask { done: false, text });
                }
            }
            _ => return false,
        }
        self.error = None;
        true
    }
}

/// The edit form for `task`, with the project list from `tasks`.
pub fn open_form(task: &Task, tasks: &[Task], week_start: Weekday) -> EditForm {
    EditForm {
        projects: combo_box::State::new(open_projects(tasks)),
        week_start,
        ..EditForm::from_task(task)
    }
}

impl DayView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces what the view shows. `main.rs` calls this after EVERY
    /// message (and on the minute tick), so it must be cheap: if `data`
    /// equals what the view already has, return early. It keeps in-progress
    /// UI state (an open edit form, typed text, the selection) unless the
    /// thing it belongs to is gone.
    ///
    /// The open form closes when its task leaves the screen or is deleted;
    /// if it moved into a collapsed section, that section opens. When the
    /// selected task leaves the visible rows, the selection moves to the
    /// next row that was below it (or above it, at the end), or none.
    pub fn load(&mut self, data: DayData) {
        if self.data.as_ref() == Some(&data) {
            return;
        }
        let before = self.visible();
        // Unsaved typing wins over what is stored.
        if self.notes_edited.is_none() {
            let stored = day_notes(&data, data.today);
            if self.notes.text().trim_end() != stored {
                self.notes = text_editor::Content::with_text(&stored);
            }
        }
        self.data = Some(data);

        if let Some(form) = &self.editing {
            match self.section_of_id(form.id) {
                None | Some(Section::Deleted) => self.editing = None,
                Some(Section::Done) => self.show_done = true,
                Some(Section::NoDate) => self.show_no_date = true,
                Some(_) => {}
            }
        }

        if let Some(selected) = self.selected {
            let after = self.visible();
            if !after.contains(&selected) {
                self.selected = before.iter().position(|&id| id == selected).and_then(|i| {
                    before[i + 1..]
                        .iter()
                        .chain(before[..i].iter().rev())
                        .find(|id| after.contains(id))
                        .copied()
                });
            }
        }
    }

    /// A key pressed while no text field has focus (PLAN.md 4.4: ↑ ↓ move
    /// the selection, Space toggles done, Enter expands, Escape closes).
    /// The selection stops at the first and last rows (no wrapping).
    pub fn on_key(&mut self, key: NavKey) -> Action {
        match key {
            NavKey::Up | NavKey::Down => {
                let rows = self.visible();
                let current = self
                    .selected
                    .and_then(|id| rows.iter().position(|&r| r == id));
                let last = rows.len().checked_sub(1);
                let next = match (key, current) {
                    (NavKey::Down, None) => rows.first().map(|_| 0),
                    (NavKey::Down, Some(i)) => last.map(|l| (i + 1).min(l)),
                    (_, None) => last,
                    (_, Some(i)) => Some(i.saturating_sub(1)),
                };
                self.selected = next.map(|i| rows[i]);
                Action::None
            }
            NavKey::Toggle => match self.selected.and_then(|id| self.task(id)) {
                Some(task) => Action::SetStatus(task.id, toggled(task.status)),
                None => Action::None,
            },
            NavKey::Open => {
                if let Some(task) = self.selected.and_then(|id| self.task(id))
                    && self.editing.as_ref().is_none_or(|f| f.id != task.id)
                    && let Some(data) = &self.data
                {
                    self.editing = Some(open_form(task, &data.tasks, data.week_start));
                }
                Action::None
            }
            NavKey::Close => {
                if self.editing.is_some() {
                    self.editing = None;
                } else {
                    self.selected = None;
                }
                Action::None
            }
        }
    }

    pub fn update(&mut self, message: Message) -> Action {
        let Some(data) = &self.data else {
            return Action::None;
        };
        if let Some(form) = self.editing.as_mut()
            && form.handle(&message, data.today)
        {
            return Action::None;
        }
        match message {
            Message::Toggle(id, done) => Action::SetStatus(
                id,
                if done {
                    TaskStatus::Done
                } else {
                    TaskStatus::Todo
                },
            ),
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
            Message::Enter(spot) => {
                if let Spot::Before(id) = spot {
                    self.hovered = Some(id);
                }
                if let Some(drag) = self.drag.as_mut().filter(|d| d.active) {
                    drag.over = Some(spot);
                }
                Action::None
            }
            Message::Leave(id) => {
                if self.hovered == Some(id) {
                    self.hovered = None;
                }
                Action::None
            }
            Message::DropOn(spot) => {
                let Some(drag) = self.drag.take().filter(|d| d.active) else {
                    return Action::None;
                };
                let order: Vec<&Task> = section_tasks(data, Section::Day);
                reorder(&order, drag.id, spot)
                    .map(Action::Reorder)
                    .unwrap_or(Action::None)
            }
            // A press let go before it became a drag is a click.
            Message::EndDrag => match self.drag.take() {
                Some(drag) if !drag.active => self.update(Message::Expand(drag.id)),
                _ => Action::None,
            },
            Message::Remove(id) => Action::SetStatus(id, TaskStatus::Dropped),
            Message::Expand(id) => {
                if self.editing.as_ref().is_some_and(|f| f.id == id) {
                    self.editing = None;
                    self.selected = None;
                } else if let Some(task) = data.tasks.iter().find(|t| t.id == id) {
                    self.editing = Some(open_form(task, &data.tasks, data.week_start));
                    self.selected = Some(id);
                }
                Action::None
            }
            Message::EditQuickAdd(s) => {
                self.quick_add = s;
                Action::None
            }
            Message::SubmitQuickAdd => {
                let new = crate::top_bar::quick_add_task(
                    &self.quick_add,
                    crate::top_bar::Tab::Today,
                    data.day,
                    data.today,
                );
                match new {
                    Some(new) => {
                        self.quick_add.clear();
                        Action::Add(new)
                    }
                    None => Action::None,
                }
            }
            Message::EditDayNotes(action) => {
                let edit = action.is_edit();
                self.notes.perform(action);
                if edit {
                    self.notes_edited = Some(Instant::now());
                }
                Action::None
            }
            Message::Save => {
                let Some(form) = self.editing.as_mut() else {
                    return Action::None;
                };
                let Some(task) = data.tasks.iter().find(|t| t.id == form.id) else {
                    self.editing = None;
                    return Action::None;
                };
                match form.apply(task) {
                    Ok(updated) => {
                        self.editing = None;
                        self.selected = None;
                        if &updated == task {
                            Action::None
                        } else {
                            Action::Update(updated)
                        }
                    }
                    Err(e) => {
                        form.error = Some(e);
                        Action::None
                    }
                }
            }
            Message::Delete => match self.editing.take() {
                Some(form) => Action::SetStatus(form.id, TaskStatus::Dropped),
                None => Action::None,
            },
            Message::Close => {
                self.editing = None;
                self.selected = None;
                Action::None
            }
            Message::Restore(id) => Action::SetStatus(id, TaskStatus::Todo),
            Message::DeleteForever(id) => Action::Delete(id),
            Message::ToggleShowDone => {
                self.show_done = !self.show_done;
                Action::None
            }
            Message::ToggleShowNoDate => {
                self.show_no_date = !self.show_no_date;
                Action::None
            }
            Message::ToggleShowDeleted => {
                self.show_deleted = !self.show_deleted;
                Action::None
            }
            // Form edits with no form open.
            _ => Action::None,
        }
    }

    fn task(&self, id: TaskId) -> Option<&Task> {
        self.data.as_ref()?.tasks.iter().find(|t| t.id == id)
    }

    fn section_of_id(&self, id: TaskId) -> Option<Section> {
        let data = self.data.as_ref()?;
        placed_section(data, self.task(id)?)
    }

    fn visible(&self) -> Vec<TaskId> {
        self.data
            .as_ref()
            .map(|d| visible_rows(d, self.show_done, self.show_no_date))
            .unwrap_or_default()
    }

    // ------------------------------------------------------------- view

    pub fn view(&self) -> Element<'_, Message> {
        let Some(data) = &self.data else {
            return text("Loading…").style(theme::muted).into();
        };
        let mut content = Column::new().spacing(theme::SPACING * 2.0);

        let recommended = section_tasks(data, Section::Recommended);
        if !recommended.is_empty() {
            let heading = section_title(&recommended, "Recommended Task", text::default);
            content = content.push(self.group(heading, recommended, data));
        } else if data.recommending && data.day == data.today {
            content = content.push(
                column![
                    text("Recommended Tasks").size(theme::TEXT_HEADING),
                    text("Picking today's top tasks…")
                        .size(theme::TEXT_BODY)
                        .style(theme::muted),
                ]
                .spacing(theme::SPACING / 2.0),
            );
        }

        // The day's tasks, then the add-a-task input as the last card.
        let day_tasks = section_tasks(data, Section::Day);
        let quick_add = text_input("Add a task", &self.quick_add)
            .id(crate::top_bar::QUICK_ADD_ID)
            .on_input(Message::EditQuickAdd)
            .on_submit(Message::SubmitQuickAdd)
            .padding([11, 14])
            .size(theme::TEXT_BODY)
            .style(theme::input);
        let heading = section_title(&day_tasks, "Task", text::default);
        let day_group = if day_tasks.is_empty() {
            column![heading]
        } else {
            column![heading, self.sortable_rows(day_tasks, data)]
        };
        content = content.push(day_group.push(quick_add).spacing(theme::SPACING / 2.0));

        let overdue = section_tasks(data, Section::Overdue);
        if !overdue.is_empty() {
            let heading = section_title(&overdue, "Overdue Task", theme::danger);
            content = content.push(self.group(heading, overdue, data));
        }

        if let Some(notes) = self.day_notes_view(data) {
            content = content.push(notes);
        }

        let collapsibles = [
            (
                Section::Done,
                "Completed",
                self.show_done,
                Message::ToggleShowDone,
            ),
            (
                Section::NoDate,
                "No date",
                self.show_no_date,
                Message::ToggleShowNoDate,
            ),
            (
                Section::Deleted,
                "Deleted",
                self.show_deleted,
                Message::ToggleShowDeleted,
            ),
        ];
        for (section, label, open, toggle) in collapsibles {
            let tasks = section_tasks(data, section);
            if !tasks.is_empty() {
                content = content.push(self.collapsible(section, label, open, toggle, tasks, data));
            }
        }

        let page = container(
            content
                .padding(theme::SPACING * 2.0)
                .max_width(theme::CONTENT_WIDTH),
        )
        .center_x(Fill);
        let mut layers = stack![page];
        if let Some(drag) = self.dragging()
            && let Some(at) = drag.at
            && let Some(task) = data.tasks.iter().find(|t| t.id == drag.id)
        {
            let ghost = container(card_content(task, data.today, None))
                .padding([11, 14])
                .width(theme::CONTENT_WIDTH - theme::SPACING * 4.0)
                .style(|t: &Theme| container::Style {
                    shadow: iced::Shadow {
                        color: Color::BLACK.scale_alpha(0.35),
                        offset: iced::Vector::new(0.0, 6.0),
                        blur_radius: 16.0,
                    },
                    ..card_box(true, false)(t)
                });
            layers = layers.push(pin(ghost).position(Point::new(at.x - 40.0, at.y - 22.0)));
        }
        let mut area = mouse_area(layers);
        if self.drag.is_some() {
            area = area.on_move(Message::Moved);
        }
        scrollable(area).height(Fill).into()
    }

    /// The open inline form's task, title and notes, for `main.rs`'s
    /// priority suggestion.
    pub fn form_text(&self) -> Option<(TaskId, String, String)> {
        self.editing
            .as_ref()
            .map(|f| (f.id, f.title.clone(), f.notes.text()))
    }

    /// Whether Today's Notes have unsaved typing (`main.rs` then checks
    /// [`Self::notes_to_save`] every so often).
    pub fn notes_pending(&self) -> bool {
        self.notes_edited.is_some()
    }

    /// Today's Notes to save, once typing has paused for
    /// [`NOTES_SAVE_DELAY`] (then they count as saved).
    pub fn notes_to_save(&mut self, now: Instant) -> Option<String> {
        let edited = self.notes_edited?;
        if now.saturating_duration_since(edited) < NOTES_SAVE_DELAY {
            return None;
        }
        self.notes_edited = None;
        Some(self.notes.text().trim_end().to_string())
    }

    /// Today's Notes on today (an editor, saved as you type), or the
    /// shown day's notes, read only, on another day that has some.
    fn day_notes_view<'a>(&'a self, data: &'a DayData) -> Option<Element<'a, Message>> {
        if data.day == data.today {
            let heading = text("Today's Notes").size(theme::TEXT_HEADING);
            let editor = text_editor(&self.notes)
                .placeholder("What's on your mind?")
                .on_action(Message::EditDayNotes)
                // Grows with the text, then scrolls.
                .min_height(120)
                .max_height(480)
                .padding([11, 14])
                .size(theme::TEXT_BODY)
                .style(theme::editor);
            return Some(
                column![heading, editor]
                    .spacing(theme::SPACING / 2.0)
                    .into(),
            );
        }
        let notes = day_notes(data, data.day);
        if notes.is_empty() {
            return None;
        }
        let body = container(text(notes).size(theme::TEXT_BODY))
            .width(Fill)
            .padding([11, 14])
            .style(|t: &Theme| {
                let b = task_card_style(false, false)(t, button::Status::Active);
                container::Style {
                    background: b.background,
                    text_color: Some(b.text_color),
                    border: b.border,
                    ..container::Style::default()
                }
            });
        Some(
            column![text("Notes").size(theme::TEXT_HEADING), body]
                .spacing(theme::SPACING / 2.0)
                .into(),
        )
    }

    /// Shows a suggested priority on the inline form of task `id`.
    pub fn set_suggested(&mut self, id: TaskId, priority: Option<Priority>) {
        if let Some(form) = self.editing.as_mut().filter(|f| f.id == id) {
            form.suggested = priority;
        }
    }

    /// The task being dragged (past the threshold), if any.
    fn dragging(&self) -> Option<&Drag> {
        self.drag.as_ref().filter(|d| d.active)
    }

    /// The Tasks section's rows: open cards drag to rearrange; the open
    /// form stays as it is. A thin strip after the last card takes drops
    /// at the end.
    fn sortable_rows<'a>(
        &'a self,
        tasks: Vec<&'a Task>,
        data: &'a DayData,
    ) -> Element<'a, Message> {
        let dragging = self.dragging();
        let over = dragging.and_then(|d| d.over);
        let mut list = Column::new().spacing(3);
        for task in tasks {
            if let Some(form) = self.editing.as_ref().filter(|f| f.id == task.id) {
                list = list.push(edit_card(task, form, data.today));
                continue;
            }
            let id = task.id;
            let held = dragging.is_some_and(|d| d.id == id);
            let line = !held && over == Some(Spot::Before(id));
            let card = container(card_content(task, data.today, None))
                .padding([11, 14])
                .width(Fill)
                .style(card_box(
                    held || (dragging.is_none() && self.hovered == Some(id)),
                    self.selected == Some(id),
                ));
            let mut area = mouse_area(column![drop_line(line), card].spacing(3))
                .on_press(Message::Press(id))
                .on_enter(Message::Enter(Spot::Before(id)))
                .on_exit(Message::Leave(id))
                .interaction(if dragging.is_some() {
                    mouse::Interaction::Grabbing
                } else {
                    mouse::Interaction::Pointer
                });
            if dragging.is_some() {
                area = area.on_release(Message::DropOn(Spot::Before(id)));
            }
            list = list.push(area);
        }
        let mut end = mouse_area(
            column![drop_line(over == Some(Spot::End))]
                .height(12)
                .width(Fill),
        );
        if dragging.is_some() {
            end = end
                .on_enter(Message::Enter(Spot::End))
                .on_release(Message::DropOn(Spot::End));
        }
        list.push(end).into()
    }

    fn group<'a>(
        &'a self,
        heading: Element<'a, Message>,
        tasks: Vec<&'a Task>,
        data: &'a DayData,
    ) -> Element<'a, Message> {
        column![heading, self.rows(tasks, data)]
            .spacing(theme::SPACING / 2.0)
            .into()
    }

    fn collapsible<'a>(
        &'a self,
        section: Section,
        label: &'a str,
        open: bool,
        toggle: Message,
        tasks: Vec<&'a Task>,
        data: &'a DayData,
    ) -> Element<'a, Message> {
        let arrow = if open { "▾" } else { "▸" };
        let toggle =
            button(text(format!("{arrow}  {label}  {}", tasks.len())).size(theme::TEXT_SMALL))
                .padding([4, 0])
                .style(theme::text_button)
                .on_press(toggle);
        let mut header = row![toggle].align_y(Center);
        if open && section == Section::Deleted {
            header = header.push(Space::new().width(Fill)).push(
                button(text("Delete all").size(theme::TEXT_SMALL))
                    .padding([4, 10])
                    .style(theme::danger_button)
                    .on_press(Message::DeleteAll),
            );
            // Lines Delete all up with the cards' Delete forever.
            header = header.padding(iced::Padding::ZERO.right(14));
        }
        let mut group = column![header].spacing(theme::SPACING / 2.0);
        if open {
            if section == Section::Deleted {
                group = group
                    .push(Column::with_children(tasks.into_iter().map(deleted_row)).spacing(6));
            } else {
                group = group.push(self.rows(tasks, data));
            }
        }
        group.into()
    }

    fn rows<'a>(&'a self, tasks: Vec<&'a Task>, data: &'a DayData) -> Element<'a, Message> {
        Column::with_children(tasks.into_iter().map(|task| match &self.editing {
            Some(form) if form.id == task.id => edit_card(task, form, data.today),
            _ => {
                let rec = data
                    .recommended
                    .iter()
                    .filter(|_| data.day == data.today)
                    .position(|r| r.id == task.id)
                    .map(|i| i + 1);
                task_row(task, data.today, self.selected == Some(task.id), rec)
            }
        }))
        .spacing(6)
        .into()
    }
}

/// A section title: the count and what they are (`5 Tasks`, `1 Overdue
/// Task`) in the heading size.
fn section_title<'a>(
    tasks: &[&Task],
    noun: &str,
    style: fn(&Theme) -> text::Style,
) -> Element<'a, Message> {
    text(count_label(tasks.len(), noun))
        .size(theme::TEXT_HEADING)
        .style(style)
        .into()
}

/// `No Tasks`, `1 Task`, `5 Tasks`: a count and a noun, plural unless the
/// count is 1.
pub fn count_label(n: usize, noun: &str) -> String {
    match n {
        0 => format!("No {noun}s"),
        1 => format!("1 {noun}"),
        _ => format!("{n} {noun}s"),
    }
}

/// A day's notes: the free-standing log lines dated `day`, oldest first,
/// one per line. (Side work logged before the notes existed shows up as
/// lines of them.)
pub fn day_notes(data: &DayData, day: Date) -> String {
    data.logs
        .iter()
        .filter(|l| l.task_id.is_none() && l.date == day)
        .map(|l| l.text.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// How long typing must pause before Today's Notes are saved.
pub const NOTES_SAVE_DELAY: Duration = Duration::from_millis(1200);

/// The background moved `amount` (0–1) towards the text colour.
fn tinted(theme: &Theme, amount: f32) -> Color {
    let p = theme.palette();
    Color::from_rgb(
        p.background.r + (p.text.r - p.background.r) * amount,
        p.background.g + (p.text.g - p.background.g) * amount,
        p.background.b + (p.text.b - p.background.b) * amount,
    )
}

/// A task card, as in the Week tab: a raised fill, lighter on hover,
/// quieter when done, with an accent ring when selected.
fn task_card_style(done: bool, selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let amount = match (done, hovered || selected) {
            (true, false) => 0.035,
            (true, true) => 0.06,
            (false, false) => 0.065,
            (false, true) => 0.095,
        };
        button::Style {
            background: Some(tinted(theme, amount).into()),
            text_color: theme.palette().text,
            border: border::rounded(theme::RADIUS).width(1).color(if selected {
                theme::accent_color().scale_alpha(0.7)
            } else {
                Color::TRANSPARENT
            }),
            ..button::Style::default()
        }
    }
}

/// What a task card shows: checkbox, title and details. `rank` is its place
/// when it is recommended: a numbered badge before the title. (The model's
/// reason isn't shown: the priority marks and the list it is on say it.)
/// The details on the right (subtasks, estimate, due day) are separated by
/// muted dots.
fn card_content(task: &Task, today: Date, rank: Option<usize>) -> Row<'_, Message> {
    let id = task.id;
    let open = task.status.is_open();

    let mut line = Row::new().spacing(theme::SPACING * 0.75).align_y(Center);
    if let Some(rank) = rank {
        line = line.push(rank_badge(rank));
    }
    // The title, then `!` for Medium or `!!` for High (open tasks only).
    let (level, style): (u8, fn(&Theme) -> text::Style) = if open {
        (priority_level(task.priority), text::default)
    } else {
        (0, theme::muted)
    };
    line = line.push(theme::title_with_marks(
        task.title.as_str(),
        level,
        style,
        |t| t.size(theme::TEXT_BODY),
    ));
    if let Some(project) = &task.project {
        line = line.push(project_chip(project));
    }
    line = line.push(Space::new().width(Fill));
    let mut details: Vec<Element<'_, Message>> = Vec::new();
    let (done, total) = subtask_counts(&task.notes);
    if total > 0 {
        details.push(
            text(format!("{done}/{total}"))
                .size(theme::TEXT_SMALL)
                .style(theme::muted)
                .into(),
        );
    }
    if let Some(minutes) = task.estimate_minutes {
        details.push(
            text(parse::format_estimate(minutes))
                .size(theme::TEXT_SMALL)
                .style(theme::muted)
                .into(),
        );
    }
    if open && let Some(due) = task.due_on {
        details.push(
            text(due_label(due, today))
                .size(theme::TEXT_SMALL)
                .style(theme::due_style(task.is_overdue(today)))
                .into(),
        );
    }
    if !details.is_empty() {
        let mut meta = Row::new().spacing(theme::SPACING / 2.0).align_y(Center);
        for (i, detail) in details.into_iter().enumerate() {
            if i > 0 {
                meta = meta.push(text("·").size(theme::TEXT_SMALL).style(theme::muted));
            }
            meta = meta.push(detail);
        }
        line = line.push(meta);
    }

    // A completed task gets a quiet × to delete it. Like the checkbox, it
    // captures its own clicks, so they don't open the form.
    if task.is_done() {
        line = line.push(
            button(text("×").size(theme::TEXT_BODY))
                .padding([0, 6])
                .style(theme::text_button)
                .on_press(Message::Remove(id)),
        );
    }
    row![
        checkbox(task.is_done()).on_toggle(move |b| Message::Toggle(id, b)),
        line,
    ]
    .spacing(theme::SPACING * 0.75)
    .align_y(Center)
}

/// A task card that opens its form when clicked (the sections that don't
/// drag).
fn task_row<'a>(
    task: &'a Task,
    today: Date,
    selected: bool,
    rank: Option<usize>,
) -> Element<'a, Message> {
    button(card_content(task, today, rank))
        .width(Fill)
        .padding([11, 14])
        .style(task_card_style(!task.status.is_open(), selected))
        .on_press(Message::Expand(task.id))
        .into()
}

/// An open Tasks card's fill, as [`task_card_style`] gives a button:
/// lighter when `lit` (hovered or held), with an accent ring when selected.
fn card_box(lit: bool, selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let status = if lit {
            button::Status::Hovered
        } else {
            button::Status::Active
        };
        let b = task_card_style(false, selected)(theme, status);
        container::Style {
            background: b.background,
            text_color: Some(b.text_color),
            border: b.border,
            ..container::Style::default()
        }
    }
}

/// The accent line where a dragged card would land (empty space otherwise).
fn drop_line<'a>(show: bool) -> Element<'a, Message> {
    container(Space::new().height(3))
        .width(Fill)
        .style(move |_: &Theme| container::Style {
            background: show.then(|| theme::accent_color().into()),
            border: border::rounded(2),
            ..container::Style::default()
        })
        .into()
}

/// The new positions after moving `dragged` to `spot` in `order` (the
/// Tasks section as shown): 1, 2, 3, … top to bottom, only for the tasks
/// whose position changes. `None` if `dragged` isn't in the list or
/// nothing moves.
pub fn reorder(order: &[&Task], dragged: TaskId, spot: Spot) -> Option<Vec<(TaskId, f64)>> {
    let moving = order.iter().find(|t| t.id == dragged)?;
    let mut ids: Vec<&Task> = order.iter().copied().filter(|t| t.id != dragged).collect();
    let at = match spot {
        Spot::Before(id) => ids.iter().position(|t| t.id == id).unwrap_or(ids.len()),
        Spot::End => ids.len(),
    };
    ids.insert(at, moving);
    let changes: Vec<(TaskId, f64)> = ids
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id, (i + 1) as f64))
        .filter(|(id, pos)| order.iter().any(|t| t.id == *id && t.position != *pos))
        .collect();
    let moved = ids.iter().map(|t| t.id).ne(order.iter().map(|t| t.id));
    (moved || !changes.is_empty())
        .then_some(changes)
        .filter(|c| !c.is_empty())
}

/// A small round number in the accent colour: a recommended task's rank.
fn rank_badge<'a>(rank: usize) -> Element<'a, Message> {
    container(
        text(rank.to_string())
            .size(12)
            .color(Color::WHITE)
            .align_x(Center),
    )
    .center_x(20)
    .center_y(20)
    .style(|_: &Theme| container::Style {
        background: Some(theme::accent_color().into()),
        border: border::rounded(999),
        ..container::Style::default()
    })
    .into()
}

/// A deleted task: a quiet card (as a completed one) with its title,
/// dimmed, and Restore and Delete forever.
fn deleted_row(task: &Task) -> Element<'_, Message> {
    let line = row![
        text(&task.title)
            .size(theme::TEXT_BODY)
            .style(theme::muted)
            .width(Fill),
        button(text("Restore").size(theme::TEXT_SMALL))
            .padding([4, 10])
            .style(theme::secondary_button)
            .on_press(Message::Restore(task.id)),
        button(text("Delete forever").size(theme::TEXT_SMALL))
            .padding([4, 10])
            .style(theme::danger_button)
            .on_press(Message::DeleteForever(task.id)),
    ]
    .spacing(theme::SPACING / 2.0)
    .align_y(Center);
    container(line)
        .width(Fill)
        .padding([7, 14])
        .style(|t: &Theme| {
            let b = task_card_style(true, false)(t, button::Status::Active);
            container::Style {
                background: b.background,
                text_color: Some(b.text_color),
                border: b.border,
                ..container::Style::default()
            }
        })
        .into()
}

fn project_chip(project: &str) -> Element<'_, Message> {
    // `theme::chip`, a step lighter so it shows on a task card.
    container(text(format!("#{project}")).size(theme::TEXT_SMALL))
        .padding(theme::chip_padding())
        .style(|theme: &Theme| container::Style {
            background: Some(tinted(theme, 0.13).into()),
            ..theme::chip(theme)
        })
        .into()
}

fn labelled<'a>(label: &'a str, field: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    column![
        text(label).size(theme::TEXT_SMALL).style(theme::muted),
        field.into()
    ]
    .spacing(4)
    .width(Fill)
    .into()
}

fn field<'a>(
    placeholder: &'a str,
    value: &'a str,
    on_input: fn(String) -> Message,
) -> text_input::TextInput<'a, Message> {
    text_input(placeholder, value)
        .on_input(on_input)
        .on_submit(Message::Save)
        .padding([8, 10])
        .size(theme::TEXT_BODY)
        .style(theme::input)
}

/// Low, Medium, High as a segmented control. A `suggested` level other
/// than the current one gets a faint accent outline.
fn priority_picker<'a>(current: Priority, suggested: Option<Priority>) -> Element<'a, Message> {
    let buttons = PRIORITIES.into_iter().map(move |(label, p)| {
        let hint = p != current && suggested.map(priority_level) == Some(p);
        let b = button(text(label).size(theme::TEXT_SMALL))
            .padding(theme::tab_padding())
            .on_press(Message::SetPriority(p));
        if hint {
            b.style(suggested_tab).into()
        } else {
            b.style(theme::tab_button(current == p)).into()
        }
    });
    container(Row::with_children(buttons).spacing(2))
        .padding(3)
        .style(theme::tab_bar)
        .into()
}

/// An unselected priority the decision model suggests: a faint outline.
fn suggested_tab(t: &Theme, status: button::Status) -> button::Style {
    let mut style = theme::tab_button(false)(t, status);
    style.border = border::rounded(theme::RADIUS - 2.0)
        .color(theme::accent_color().scale_alpha(0.6))
        .width(1.0);
    style
}

/// The due-date field: a button showing the date that opens the calendar.
fn due_button(due: Option<Date>, today: Date) -> Element<'static, Message> {
    let label = match due {
        Some(d) if d.year() == today.year() => d.strftime("%a %-d %b").to_string(),
        Some(d) => d.strftime("%a %-d %b %Y").to_string(),
        None => String::new(),
    };
    button(text(label).size(theme::TEXT_BODY))
        .width(Fill)
        .height(35)
        .padding([8, 10])
        .style(field_button)
        .on_press(Message::ToggleCalendar)
        .into()
}

/// A button that looks like [`theme::input`].
fn field_button(theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(tinted(theme, if hovered { 0.11 } else { 0.075 }).into()),
        text_color: theme.palette().text,
        border: border::rounded(theme::RADIUS),
        ..button::Style::default()
    }
}

/// A subtask's card: the input fill, quieter when done.
fn subtask_card(done: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| container::Style {
        background: Some(tinted(theme, if done { 0.05 } else { 0.075 }).into()),
        border: border::rounded(theme::RADIUS),
        ..container::Style::default()
    }
}

fn subtasks_view(form: &EditForm) -> Element<'_, Message> {
    let mut list = Column::new().spacing(6);
    for (i, s) in form.subtasks.iter().enumerate() {
        let mut label = text(&s.text).size(theme::TEXT_BODY).width(Fill);
        if s.done {
            label = label.style(theme::muted);
        }
        list = list.push(
            container(
                row![
                    checkbox(s.done).on_toggle(move |b| Message::ToggleSubtask(i, b)),
                    label,
                    button(text("×").size(theme::TEXT_BODY))
                        .padding([0, 8])
                        .style(theme::text_button)
                        .on_press(Message::RemoveSubtask(i)),
                ]
                .spacing(theme::SPACING * 0.75)
                .align_y(Center),
            )
            .padding([7, 12])
            .width(Fill)
            .style(subtask_card(s.done)),
        );
    }
    list.push(
        text_input("Add a subtask", &form.new_subtask)
            .on_input(Message::EditNewSubtask)
            .on_submit(Message::AddSubtask)
            .padding([9, 12])
            .size(theme::TEXT_BODY)
            .style(theme::input),
    )
    .into()
}

/// The project field: type a name, or pick one from the projects of open
/// tasks, listed when the field is clicked.
fn project_field(form: &EditForm) -> Element<'_, Message> {
    let current = (!form.project.is_empty()).then_some(&form.project);
    combo_box(&form.projects, &form.project, current, Message::PickProject)
        .on_input(Message::EditProject)
        .padding([8, 10])
        .size(theme::TEXT_BODY)
        .input_style(theme::input)
        .menu_style(project_menu)
        .into()
}

/// The project list: a raised panel like the calendar.
fn project_menu(theme: &Theme) -> menu::Style {
    let p = theme.palette();
    menu::Style {
        background: tinted(theme, 0.06).into(),
        border: border::rounded(theme::RADIUS)
            .width(1)
            .color(theme::muted_color(theme).scale_alpha(0.25)),
        text_color: p.text,
        selected_text_color: Color::WHITE,
        selected_background: theme::accent_color().into(),
        shadow: iced::Shadow {
            color: Color::BLACK.scale_alpha(0.3),
            offset: iced::Vector::new(0.0, 6.0),
            blur_radius: 18.0,
        },
    }
}

/// The form's fields, under `title_row` (the title field, with a checkbox
/// for an existing task): project, due, estimate, priority, the error,
/// notes and subtasks.
fn form_fields<'a>(
    form: &'a EditForm,
    today: Date,
    title_row: Element<'a, Message>,
) -> Column<'a, Message> {
    let notes = text_editor(&form.notes)
        .placeholder("Notes")
        .on_action(Message::EditNotes)
        .height(96)
        .padding(10)
        .size(theme::TEXT_BODY)
        .style(theme::editor);

    let mut body = column![
        title_row,
        row![
            labelled("Project", project_field(form)),
            labelled(
                "Due",
                Popup::new(
                    due_button(form.due, today),
                    form.calendar.map(|month| month_picker(
                        month,
                        form.due,
                        None,
                        today,
                        form.week_start,
                        Message::CalendarMonth,
                        Message::PickDue,
                        Some(Message::ClearDue),
                    )),
                    Message::CloseCalendar,
                )
            ),
            labelled(
                "Estimate",
                field("1h30m", &form.estimate, Message::EditEstimate)
            ),
        ]
        .spacing(theme::SPACING),
    ]
    .spacing(theme::SPACING);

    body = body.push(labelled(
        "Priority",
        priority_picker(form.priority, form.suggested),
    ));
    if let Some(error) = &form.error {
        body = body.push(text(error).size(theme::TEXT_SMALL).style(theme::danger));
    }
    body = body.push(labelled("Notes", notes));
    body.push(labelled("Subtasks", subtasks_view(form)))
}

/// The id of the new-task dialog's title field, for `operation::focus`.
pub const DIALOG_TITLE_ID: &str = "new-task-title";

/// The task dialog's panel (the Week tab): a heading naming the day, the
/// form, then the buttons. For a new task (`editing` false): Cancel and Add
/// task. For an existing one: Delete on the left, Cancel and Save on the
/// right. `Save` adds or saves, `Close` cancels, `Delete` deletes.
/// `main.rs` shows it over everything.
pub fn task_dialog(
    form: &EditForm,
    day: Option<Date>,
    editing: bool,
    today: Date,
) -> Element<'_, Message> {
    let day_label = day.map_or_else(
        || "No date".to_string(),
        |d| d.strftime("%A %-d %B").to_string(),
    );
    let heading = row![
        text(if editing { "Edit task" } else { "New task" }).size(theme::TEXT_HEADING),
        text(day_label).size(theme::TEXT_BODY).style(theme::muted),
    ]
    .spacing(theme::SPACING)
    .align_y(iced::alignment::Vertical::Bottom);
    let title = field("Title", &form.title, Message::EditTitle)
        .id(DIALOG_TITLE_ID)
        .into();
    let body = column![heading, form_fields(form, today, title)]
        .spacing(theme::SPACING * 1.5)
        .push({
            let mut buttons = Row::new().spacing(theme::SPACING / 2.0).align_y(Center);
            if editing {
                buttons = buttons.push(
                    button(text("Delete").size(theme::TEXT_BODY))
                        .padding([6, 12])
                        .style(theme::danger_button)
                        .on_press(Message::Delete),
                );
            }
            buttons
                .push(Space::new().width(Fill))
                .push(
                    button(text("Cancel").size(theme::TEXT_BODY))
                        .padding([6, 12])
                        .style(theme::text_button)
                        .on_press(Message::Close),
                )
                .push(
                    button(text(if editing { "Save" } else { "Add task" }).size(theme::TEXT_BODY))
                        .padding([6, 16])
                        .style(theme::primary_button)
                        .on_press(Message::Save),
                )
        });
    container(body).padding(theme::SPACING * 2.0).into()
}

fn edit_card<'a>(task: &'a Task, form: &'a EditForm, today: Date) -> Element<'a, Message> {
    let id = task.id;
    let title = row![
        checkbox(task.is_done()).on_toggle(move |b| Message::Toggle(id, b)),
        field("Title", &form.title, Message::EditTitle),
    ]
    .spacing(theme::SPACING / 2.0)
    .align_y(Center)
    .into();
    let mut body = form_fields(form, today, title);

    body = body.push(
        row![
            button(text("Delete").size(theme::TEXT_BODY))
                .padding([6, 12])
                .style(theme::danger_button)
                .on_press(Message::Delete),
            Space::new().width(Fill),
            button(text("Close").size(theme::TEXT_BODY))
                .padding([6, 12])
                .style(theme::text_button)
                .on_press(Message::Close),
            button(text("Save").size(theme::TEXT_BODY))
                .padding([6, 16])
                .style(theme::primary_button)
                .on_press(Message::Save),
        ]
        .spacing(theme::SPACING / 2.0)
        .align_y(Center),
    );

    // A little air above and below so the card doesn't touch its
    // neighbours (rows sit 2 px apart).
    container(
        container(body)
            .padding(theme::SPACING * 1.25)
            .width(Fill)
            .style(theme::card),
    )
    .padding([theme::SPACING / 2.0, 0.0])
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    /// Monday 16 November 2026.
    fn today() -> Date {
        date(2026, 11, 16)
    }

    /// Due and planned on `due` (as migration 3 leaves older tasks).
    fn task(id: TaskId, title: &str, due: Option<Date>) -> Task {
        Task {
            id,
            title: title.to_string(),
            due_on: due,
            planned_for: due,
            position: id as f64,
            ..Task::default()
        }
    }

    fn with_status(mut t: Task, status: TaskStatus) -> Task {
        t.status = status;
        t
    }

    fn planned(mut t: Task, day: Date) -> Task {
        t.planned_for = Some(day);
        t
    }

    /// Completed at noon UTC on `day`.
    fn done_on(t: Task, day: Date) -> Task {
        let mut t = with_status(t, TaskStatus::Done);
        t.completed_at = Some(TimeZone::UTC.to_timestamp(day.at(12, 0, 0, 0)).unwrap());
        t
    }

    fn data_for(day: Date, tasks: Vec<Task>) -> DayData {
        DayData {
            day,
            today: today(),
            tz: TimeZone::UTC,
            tasks,
            week_start: Weekday::Monday,
            recommended: Vec::new(),
            recommending: false,
            logs: Vec::new(),
        }
    }

    fn loaded(tasks: Vec<Task>) -> DayView {
        let mut view = DayView::new();
        view.load(data_for(today(), tasks));
        view
    }

    fn ids(tasks: &[&Task]) -> Vec<TaskId> {
        tasks.iter().map(|t| t.id).collect()
    }

    /// A mix covering every section on Mon 16 Nov.
    fn mixed() -> Vec<Task> {
        vec![
            task(1, "overdue", Some(date(2026, 11, 13))),
            task(2, "today", Some(today())),
            // Planned for last Friday, due next week: carried over.
            planned(
                task(3, "carried", Some(date(2026, 11, 25))),
                date(2026, 11, 13),
            ),
            // Planned today, deadline passed: overdue, not twice.
            planned(
                task(4, "planned overdue", Some(date(2026, 11, 10))),
                today(),
            ),
            done_on(task(5, "done today", Some(today())), today()),
            with_status(task(6, "deleted", Some(today())), TaskStatus::Dropped),
            task(7, "inbox", None),
            task(8, "wednesday", Some(date(2026, 11, 18))),
            done_on(
                task(9, "done wed", Some(date(2026, 11, 18))),
                date(2026, 11, 18),
            ),
            // Due last week, ticked today: done on its own day, not today.
            done_on(task(10, "late but done", Some(date(2026, 11, 9))), today()),
            planned(task(11, "planned wed", None), date(2026, 11, 18)),
            with_status(task(12, "doing today", Some(today())), TaskStatus::Doing),
        ]
    }

    #[test]
    fn completed_tasks_delete_with_the_cross_and_forms_report_their_text() {
        let mut view = loaded(mixed());
        assert_eq!(
            view.update(Message::Remove(5)),
            Action::SetStatus(5, TaskStatus::Dropped)
        );
        let _ = view.view();
        assert_eq!(view.form_text(), None);
        view.update(Message::Expand(2));
        assert_eq!(view.form_text(), Some((2, "today".into(), String::new())));
        view.set_suggested(2, Some(2));
        assert_eq!(view.editing.as_ref().and_then(|f| f.suggested), Some(2));
        // Another task's suggestion doesn't land on this form.
        view.set_suggested(3, Some(0));
        assert_eq!(view.editing.as_ref().and_then(|f| f.suggested), Some(2));
    }

    #[test]
    fn reorder_renumbers_the_section_top_to_bottom() {
        let a = planned(task(1, "a", None), today());
        let b = planned(task(2, "b", None), today());
        let c = planned(task(3, "c", None), today());
        let order = [&a, &b, &c];
        // c before a: c a b → positions 1 2 3, all three change.
        assert_eq!(
            reorder(&order, 3, Spot::Before(1)),
            Some(vec![(3, 1.0), (1, 2.0), (2, 3.0)])
        );
        // a to the end: b c a; only what moves is written.
        assert_eq!(
            reorder(&order, 1, Spot::End),
            Some(vec![(2, 1.0), (3, 2.0), (1, 3.0)])
        );
        // Dropped where it was: nothing to do.
        assert_eq!(reorder(&order, 2, Spot::Before(3)), None);
        assert_eq!(reorder(&order, 9, Spot::End), None);
    }

    #[test]
    fn drag_reorders_the_tasks_section_and_a_click_still_opens() {
        let mut a = planned(task(1, "a", None), today());
        let mut b = planned(task(2, "b", None), today());
        a.priority = 0;
        b.priority = 2;
        let mut view = loaded(vec![a, b]);
        // Manual order, not priority: a (position 1) before b.
        let data = view.data.clone().unwrap();
        assert_eq!(ids(&section_tasks(&data, Section::Day)), vec![1, 2]);

        // Press on b, move past the threshold, enter a, let go there.
        view.update(Message::Press(2));
        view.update(Message::Moved(Point::new(10.0, 10.0)));
        view.update(Message::Moved(Point::new(10.0, 40.0)));
        view.update(Message::Enter(Spot::Before(1)));
        assert!(view.dragging().is_some());
        let _ = view.view();
        assert_eq!(
            view.update(Message::DropOn(Spot::Before(1))),
            Action::Reorder(vec![(2, 1.0), (1, 2.0)])
        );
        assert_eq!(view.update(Message::EndDrag), Action::None);

        // A press let go without moving opens the card.
        view.update(Message::Press(1));
        view.update(Message::Moved(Point::new(5.0, 5.0)));
        view.update(Message::EndDrag);
        assert_eq!(view.editing.as_ref().map(|f| f.id), Some(1));
    }

    #[test]
    fn sections_for_today() {
        let data = data_for(today(), mixed());
        assert_eq!(ids(&section_tasks(&data, Section::Overdue)), vec![4, 1]);
        // Doing is a plain open task now. Manual order (position = id here).
        assert_eq!(ids(&section_tasks(&data, Section::Day)), vec![2, 3, 12]);
        assert_eq!(ids(&section_tasks(&data, Section::Done)), vec![5]);
        assert_eq!(ids(&section_tasks(&data, Section::NoDate)), vec![7]);
        assert_eq!(ids(&section_tasks(&data, Section::Deleted)), vec![6]);
        for id in [8, 9, 10, 11] {
            let t = data.tasks.iter().find(|t| t.id == id).unwrap();
            assert_eq!(
                section_of(t, today(), today(), &TimeZone::UTC),
                None,
                "{id}"
            );
        }
    }

    #[test]
    fn a_done_task_shows_on_its_day_not_the_day_it_was_ticked() {
        let wed = date(2026, 11, 18);
        let early = done_on(planned(task(1, "early", None), wed), today());
        let on = |day| {
            ids(&section_tasks(
                &data_for(day, vec![early.clone()]),
                Section::Done,
            ))
        };
        assert_eq!(on(wed), vec![1]);
        assert!(on(today()).is_empty());
        // No day: the day it was ticked.
        let loose = done_on(task(2, "loose", None), today());
        let loose = Task {
            planned_for: None,
            ..loose
        };
        let data = data_for(today(), vec![loose]);
        assert_eq!(ids(&section_tasks(&data, Section::Done)), vec![2]);
    }

    #[test]
    fn sections_for_another_day() {
        let wed = date(2026, 11, 18);
        let data = data_for(wed, mixed());
        assert!(section_tasks(&data, Section::Overdue).is_empty());
        assert_eq!(ids(&section_tasks(&data, Section::Day)), vec![8, 11]);
        assert_eq!(ids(&section_tasks(&data, Section::Done)), vec![9]);
        assert_eq!(ids(&section_tasks(&data, Section::NoDate)), vec![7]);
        // Deleted shows on every day.
        assert_eq!(ids(&section_tasks(&data, Section::Deleted)), vec![6]);
    }

    /// The day's tasks are in manual order: position, then id. Priority and
    /// the planned day don't reorder them.
    #[test]
    fn sort_order() {
        let mut a = task(1, "a", Some(today()));
        a.position = 5.0;
        let mut b = task(2, "b", Some(today()));
        b.position = 1.0;
        let mut c = task(3, "c", Some(today()));
        c.priority = 2;
        let e = planned(task(5, "e", None), date(2026, 11, 12));
        let mut f = task(6, "f", Some(today()));
        f.priority = 1;
        let data = data_for(today(), vec![a, b, c, e, f]);
        assert_eq!(
            ids(&section_tasks(&data, Section::Day)),
            vec![2, 3, 1, 5, 6]
        );
    }

    #[test]
    fn visible_rows_skip_collapsed_sections_and_deleted() {
        let data = data_for(today(), mixed());
        // The day's tasks, then Overdue under them.
        assert_eq!(visible_rows(&data, false, false), vec![2, 3, 12, 4, 1]);
        let all = visible_rows(&data, true, true);
        assert_eq!(all.len(), 7);
        assert_eq!(all.last(), Some(&7));
        assert!(!all.contains(&6));
    }

    #[test]
    fn due_labels() {
        let cases = [
            (date(2026, 11, 13), "3d ago"),
            (date(2026, 11, 15), "Yesterday"),
            (date(2026, 11, 16), "Today"),
            (date(2026, 11, 17), "Tomorrow"),
            (date(2026, 11, 18), "Wed"),
            (date(2026, 11, 23), "in 7d"),
            (date(2026, 11, 30), "Nov 30"),
            (date(2027, 1, 4), "Jan 4, 2027"),
        ];
        for (due, want) in cases {
            assert_eq!(due_label(due, today()), want, "{due}");
        }
    }

    #[test]
    fn keyboard_moves_toggles_and_closes() {
        let mut view = loaded(mixed());
        view.on_key(NavKey::Down);
        assert_eq!(view.selected, Some(2));
        view.on_key(NavKey::Up);
        assert_eq!(view.selected, Some(2), "stops at the top");
        assert_eq!(
            view.on_key(NavKey::Toggle),
            Action::SetStatus(2, TaskStatus::Done)
        );
        view.on_key(NavKey::Open);
        assert_eq!(view.editing.as_ref().map(|f| f.id), Some(2));
        view.on_key(NavKey::Close);
        assert!(view.editing.is_none());
        view.on_key(NavKey::Close);
        assert_eq!(view.selected, None);
    }

    #[test]
    fn delete_moves_to_deleted_and_restore_or_purge_from_there() {
        let mut view = loaded(vec![task(3, "t", None)]);
        view.update(Message::Expand(3));
        assert_eq!(
            view.update(Message::Delete),
            Action::SetStatus(3, TaskStatus::Dropped)
        );
        assert!(view.editing.is_none());
        assert_eq!(
            view.update(Message::Restore(3)),
            Action::SetStatus(3, TaskStatus::Todo)
        );
        assert_eq!(view.update(Message::DeleteForever(3)), Action::Delete(3));
    }

    #[test]
    fn form_closes_when_its_task_is_deleted_and_follows_it_to_completed() {
        let mut view = loaded(mixed());
        view.update(Message::Expand(2));
        view.update(Message::EditTitle("typed".into()));
        let mut tasks = mixed();
        let t = tasks.iter_mut().find(|t| t.id == 2).unwrap();
        *t = done_on(t.clone(), today());
        view.load(data_for(today(), tasks.clone()));
        assert_eq!(view.editing.as_ref().unwrap().title, "typed");
        assert!(view.show_done);

        tasks.iter_mut().find(|t| t.id == 2).unwrap().status = TaskStatus::Dropped;
        view.load(data_for(today(), tasks));
        assert!(view.editing.is_none());
    }

    #[test]
    fn unchanged_save_returns_none() {
        let mut t = task(4, "Same", Some(date(2026, 11, 20)));
        t.planned_for = Some(today());
        t.estimate_minutes = Some(90);
        t.priority = 3;
        t.notes = "- [x] one\nnotes\n- [ ] two".into();
        t.project = Some("work".into());
        let mut view = loaded(vec![t]);
        view.update(Message::Expand(4));
        assert_eq!(view.update(Message::Save), Action::None);
    }

    #[test]
    fn edit_form_round_trip() {
        let mut t = task(4, "Old", Some(date(2026, 11, 20)));
        t.planned_for = Some(date(2026, 11, 18));
        let mut view = loaded(vec![t.clone()]);
        view.update(Message::Expand(4));
        assert_eq!(
            view.editing.as_ref().unwrap().due,
            Some(date(2026, 11, 20)),
            "the deadline wins over the planned day"
        );
        view.update(Message::EditTitle("  New title ".into()));
        view.update(Message::EditProject("#home".into()));
        view.update(Message::ToggleCalendar);
        assert_eq!(
            view.editing.as_ref().unwrap().calendar,
            Some(date(2026, 11, 1))
        );
        view.update(Message::CalendarMonth(1));
        view.update(Message::PickDue(date(2026, 12, 2)));
        assert_eq!(view.editing.as_ref().unwrap().calendar, None);
        view.update(Message::EditEstimate("~1h30m".into()));
        view.update(Message::SetPriority(2));
        view.update(Message::EditNewSubtask("first".into()));
        view.update(Message::AddSubtask);
        view.update(Message::EditNewSubtask("second".into()));
        view.update(Message::AddSubtask);
        view.update(Message::ToggleSubtask(0, true));
        view.update(Message::EditNewSubtask("typed, not added".into()));
        view.update(Message::EditNotes(text_editor::Action::Edit(
            text_editor::Edit::Paste(std::sync::Arc::new("some notes".to_string())),
        )));
        let Action::Update(updated) = view.update(Message::Save) else {
            panic!("expected Update");
        };
        assert_eq!(
            updated,
            Task {
                title: "New title".into(),
                project: Some("home".into()),
                due_on: Some(date(2026, 12, 2)),
                estimate_minutes: Some(90),
                priority: 2,
                notes: "some notes\n\n- [x] first\n- [ ] second\n- [ ] typed, not added".into(),
                ..t
            }
        );

        let form = EditForm::from_task(&updated);
        assert_eq!(form.subtasks.len(), 3);
        assert_eq!(form.notes.text().trim(), "some notes");
        assert_eq!(form.apply(&updated).unwrap(), updated);
    }

    #[test]
    fn setting_the_due_date_keeps_the_planned_day() {
        let t = planned(task(1, "t", None), date(2026, 11, 17));
        let mut view = loaded(vec![t]);
        view.update(Message::Expand(1));
        view.update(Message::PickDue(date(2026, 11, 18)));
        let Action::Update(updated) = view.update(Message::Save) else {
            panic!("expected Update");
        };
        assert_eq!(updated.due_on, Some(date(2026, 11, 18)));
        assert_eq!(updated.planned_for, Some(date(2026, 11, 17)));

        // An inbox task stays in the inbox with a deadline.
        let mut inbox = task(2, "inbox", None);
        inbox.due_on = Some(date(2026, 11, 18));
        assert_eq!(
            section_of(&inbox, date(2026, 11, 18), today(), &TimeZone::UTC),
            Some(Section::NoDate)
        );
    }

    #[test]
    fn add_a_task_plans_it_for_the_day_shown() {
        let mut view = DayView::new();
        view.load(data_for(date(2026, 11, 18), vec![]));
        view.update(Message::EditQuickAdd("Call Sam #work".into()));
        let Action::Add(new) = view.update(Message::SubmitQuickAdd) else {
            panic!("expected Add");
        };
        assert_eq!(new.title, "Call Sam");
        assert_eq!(new.planned_for, Some(date(2026, 11, 18)));
        assert!(view.quick_add.is_empty());
        assert_eq!(view.update(Message::SubmitQuickAdd), Action::None);
    }

    #[test]
    fn blank_form_makes_a_new_task_for_the_day() {
        let mut form = EditForm::blank(&[], Weekday::Monday);
        assert!(form.new_task(today()).is_err());
        form.handle(&Message::EditTitle("Draft plan".into()), today());
        form.handle(&Message::PickDue(date(2026, 11, 20)), today());
        form.handle(&Message::EditNewSubtask("outline".into()), today());
        let new = form.new_task(date(2026, 11, 18)).unwrap();
        assert_eq!(new.title, "Draft plan");
        assert_eq!(new.planned_for, Some(date(2026, 11, 18)));
        assert_eq!(new.due_on, Some(date(2026, 11, 20)));
        assert_eq!(new.notes, "- [ ] outline");
    }

    #[test]
    fn recommended_tasks_lead_today_and_leave_the_other_sections() {
        let tasks: Vec<Task> = (1..=5)
            .map(|i| task(i, &format!("t{i}"), Some(today())))
            .chain([with_status(
                task(6, "done", Some(today())),
                TaskStatus::Done,
            )])
            .collect();
        let mut data = data_for(today(), tasks);
        data.recommended = [6, 4, 2, 9, 1, 3]
            .into_iter()
            .map(|id| Recommended {
                id,
                why: String::new(),
            })
            .collect();
        // Done and missing ids are skipped; at most three.
        fn ids(data: &DayData, s: Section) -> Vec<TaskId> {
            section_tasks(data, s).iter().map(|t| t.id).collect()
        }
        assert_eq!(ids(&data, Section::Recommended), [4, 2, 1]);
        assert_eq!(ids(&data, Section::Day), [3, 5]);
        assert_eq!(visible_rows(&data, false, false), [4, 2, 1, 3, 5]);

        // Another day: no recommendations.
        data.day = date(2026, 11, 17);
        assert!(ids(&data, Section::Recommended).is_empty());
    }

    #[test]
    fn count_labels_are_plural_unless_one() {
        assert_eq!(count_label(1, "Task"), "1 Task");
        assert_eq!(count_label(3, "Recommended Task"), "3 Recommended Tasks");
        assert_eq!(count_label(0, "Task"), "No Tasks");
    }

    #[test]
    fn project_list_is_open_tasks_only() {
        let mut a = task(1, "a", None);
        a.project = Some("work".into());
        let mut b = task(2, "b", None);
        b.project = Some("Home".into());
        let mut c = with_status(task(3, "c", None), TaskStatus::Done);
        c.project = Some("old".into());
        let mut d = task(4, "d", None);
        d.project = Some("work".into());
        assert_eq!(open_projects(&[a, b, c, d]), ["Home", "work"]);
    }

    #[test]
    fn clearing_the_due_date_and_invalid_estimate() {
        let t = task(1, "t", Some(today()));
        let mut view = loaded(vec![t]);
        view.update(Message::Expand(1));
        view.update(Message::ClearDue);
        view.update(Message::EditEstimate("lots".into()));
        assert_eq!(view.update(Message::Save), Action::None);
        let err = view.editing.as_ref().unwrap().error.clone().unwrap();
        assert!(err.contains("lots"), "{err}");
        view.update(Message::EditEstimate(String::new()));
        let Action::Update(updated) = view.update(Message::Save) else {
            panic!("expected Update");
        };
        assert_eq!(updated.due_on, None);
    }

    #[test]
    fn notes_split_and_join() {
        let (subs, text) = split_notes("intro\n- [ ] a\n  - [X] b\n- [y] not one\n");
        assert_eq!(
            subs,
            vec![
                Subtask {
                    done: false,
                    text: "a".into()
                },
                Subtask {
                    done: true,
                    text: "b".into()
                },
            ]
        );
        assert_eq!(text, "intro\n- [y] not one");
        assert_eq!(join_notes(&subs, ""), "- [ ] a\n- [x] b");
        assert_eq!(join_notes(&[], " x "), "x");
        assert_eq!(subtask_counts("- [x] a\n- [ ] b\nc"), (1, 2));
    }

    #[test]
    fn todays_notes_load_and_save_after_a_pause() {
        let mut data = data_for(today(), vec![]);
        let line = |id, date, text: &str| LogLine {
            id,
            task_id: None,
            at: Timestamp::UNIX_EPOCH,
            date,
            text: text.into(),
        };
        // Old side work lines become lines of the notes; other days' don't.
        data.logs = vec![
            line(1, today(), "helped Sam"),
            line(2, date(2026, 11, 15), "Sunday"),
            line(3, today(), "standup ran long"),
        ];
        let mut view = DayView::new();
        view.load(data.clone());
        assert_eq!(view.notes.text(), "helped Sam\nstandup ran long");
        assert!(!view.notes_pending());

        view.update(Message::EditDayNotes(text_editor::Action::Move(
            text_editor::Motion::DocumentEnd,
        )));
        assert!(!view.notes_pending(), "moving the cursor is no edit");
        view.update(Message::EditDayNotes(text_editor::Action::Edit(
            text_editor::Edit::Insert('!'),
        )));
        assert!(view.notes_pending());
        let typed = view.notes_edited.unwrap();
        assert_eq!(view.notes_to_save(typed), None, "still typing");
        // A reload while typing keeps the typing.
        view.load(data_for(today(), vec![]));
        assert_eq!(view.notes.text(), "helped Sam\nstandup ran long!");
        assert_eq!(
            view.notes_to_save(typed + NOTES_SAVE_DELAY).as_deref(),
            Some("helped Sam\nstandup ran long!")
        );
        assert!(!view.notes_pending());
        assert_eq!(view.notes_to_save(typed + NOTES_SAVE_DELAY * 2), None);
    }

    #[test]
    fn view_builds_in_every_state() {
        let mut data = data_for(today(), mixed());
        data.logs = vec![LogLine {
            id: 1,
            task_id: None,
            at: Timestamp::UNIX_EPOCH,
            date: today(),
            text: "side".into(),
        }];
        let mut view = DayView::new();
        let _ = view.view();
        view.load(data.clone());
        view.update(Message::ToggleShowDone);
        view.update(Message::ToggleShowNoDate);
        view.update(Message::ToggleShowDeleted);
        for id in [1, 12, 5, 7] {
            view.update(Message::Expand(id));
            view.update(Message::ToggleCalendar);
            let _ = view.view();
        }
        data.day = date(2026, 11, 20);
        view.load(data);
        let _ = view.view();
    }
}
