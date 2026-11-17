//! The Today tab: one day's tasks (PLAN.md 4.2 "Today").
//!
//! Sections, top to bottom:
//!
//! - **Overdue** (only when the shown day is today, and only if any): open
//!   tasks whose deadline has passed.
//! - **The day**: open tasks whose [`Task::day`] (the planned day) is the
//!   shown day. On today this also carries over open tasks planned for an
//!   earlier day whose deadline has not passed.
//! - **Side work**: free-standing log lines dated the shown day, shown as
//!   ticked items, with an input to add one (today only, see below).
//! - **Completed**, collapsed: tasks completed on the shown day.
//! - **No date**, collapsed: open tasks with no planned day (the inbox).
//! - **Deleted**, collapsed: every deleted task, newest first, each with
//!   Restore and Delete forever. Delete moves a task here (status
//!   `Dropped`); only Delete forever removes it.
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
//! clears the selection.
//!
//! The statuses `Doing` (in progress) are shown as plain open tasks; the app
//! no longer sets it.
//!
//! Skipped for the prototype: drag ordering, strikethrough on done tasks
//! (iced text has none), and adding side work to a day other than today
//! (`Action::AddLog` has no date; main.rs stamps it now).

use std::cmp::Ordering;

use fortnight_core::domain::{EventId, Priority, Task, TaskId, TaskStatus};
use fortnight_core::parse;
use fortnight_core::week::add_months;
use iced::widget::overlay::menu;
use iced::widget::{
    Column, Row, Space, button, checkbox, column, combo_box, container, row, scrollable, text,
    text_editor, text_input,
};
use iced::{Center, Color, Element, Fill, Theme, border};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};

use super::NavKey;
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
    /// The side work input.
    EditDayLog(String),
    AddDayLog,
    DeleteLog(EventId),
    Save,
    /// Move the open task to Deleted.
    Delete,
    Close,
    /// Put a deleted task back.
    Restore(TaskId),
    /// Remove a deleted task for good.
    DeleteForever(TaskId),
    ToggleShowDone,
    ToggleShowNoDate,
    ToggleShowDeleted,
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
    /// The side work input.
    day_log: String,
}

/// Where a task is shown on the Today tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Overdue,
    /// The shown day's open tasks.
    Day,
    Done,
    NoDate,
    Deleted,
}

impl Section {
    /// Display order of the task sections (side work sits between Day and
    /// Done).
    pub const ALL: [Section; 5] = [
        Section::Overdue,
        Section::Day,
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
        TaskStatus::Done => return (completed_on(task, tz) == Some(day)).then_some(Section::Done),
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

/// The tasks in `section`, sorted. Overdue: oldest deadline first, then
/// [`open_order`]. The day and No date: [`open_order`]. Completed: most
/// recently finished first. Deleted: most recently deleted first.
pub fn section_tasks(data: &DayData, section: Section) -> Vec<&Task> {
    let mut out: Vec<&Task> = data
        .tasks
        .iter()
        .filter(|t| section_of(t, data.day, data.today, &data.tz) == Some(section))
        .collect();
    match section {
        Section::Overdue => out.sort_by(|a, b| a.due_on.cmp(&b.due_on).then(open_order(a, b))),
        Section::Day | Section::NoDate => out.sort_by(|a, b| open_order(a, b)),
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

/// The heading of the day section: `Today`, `Tomorrow`, `Yesterday`, or
/// `Wed 18 Nov`.
pub fn day_heading(day: Date, today: Date) -> String {
    match days_between(today, day) {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        -1 => "Yesterday".to_string(),
        _ => day.strftime("%a %-d %b").to_string(),
    }
}

/// The text shown when the shown day has no open tasks.
pub fn empty_message(day: Date, today: Date) -> String {
    if day == today {
        "Nothing planned for today.".to_string()
    } else {
        format!("Nothing planned for {}", day.strftime("%a %-d %b"))
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
    pub estimate: String,
    /// The estimate as first shown, so an untouched field is not re-parsed.
    pub original_estimate: String,
    pub priority: Priority,
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
            estimate: estimate.clone(),
            original_estimate: estimate,
            priority: priority_level(task.priority),
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

/// The edit form for `task`, with the project list from `tasks`.
fn open_form(task: &Task, tasks: &[Task]) -> EditForm {
    EditForm {
        projects: combo_box::State::new(open_projects(tasks)),
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
                    self.editing = Some(open_form(task, &data.tasks));
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
        match message {
            Message::Toggle(id, done) => Action::SetStatus(
                id,
                if done {
                    TaskStatus::Done
                } else {
                    TaskStatus::Todo
                },
            ),
            Message::Expand(id) => {
                if self.editing.as_ref().is_some_and(|f| f.id == id) {
                    self.editing = None;
                } else if let Some(task) = data.tasks.iter().find(|t| t.id == id) {
                    self.editing = Some(open_form(task, &data.tasks));
                    self.selected = Some(id);
                }
                Action::None
            }
            Message::EditTitle(s) => self.edit(|f| f.title = s),
            Message::EditProject(s) | Message::PickProject(s) => self.edit(|f| f.project = s),
            Message::EditEstimate(s) => self.edit(|f| f.estimate = s),
            Message::EditNotes(action) => self.edit(|f| f.notes.perform(action)),
            Message::SetPriority(p) => self.edit(|f| f.priority = priority_level(p)),
            Message::ToggleCalendar => {
                let today = data.today;
                self.edit(|f| {
                    f.calendar = match f.calendar {
                        Some(_) => None,
                        None => Some(f.due.unwrap_or(today).first_of_month()),
                    }
                })
            }
            Message::CloseCalendar => self.edit(|f| f.calendar = None),
            Message::CalendarMonth(step) => self.edit(|f| {
                if let Some(month) = &mut f.calendar {
                    *month = add_months(*month, step).first_of_month();
                }
            }),
            Message::PickDue(date) => self.edit(|f| {
                f.due = Some(date);
                f.calendar = None;
            }),
            Message::ClearDue => self.edit(|f| {
                f.due = None;
                f.calendar = None;
            }),
            Message::ToggleSubtask(i, done) => self.edit(|f| {
                if let Some(s) = f.subtasks.get_mut(i) {
                    s.done = done;
                }
            }),
            Message::RemoveSubtask(i) => self.edit(|f| {
                if i < f.subtasks.len() {
                    f.subtasks.remove(i);
                }
            }),
            Message::EditNewSubtask(s) => self.edit(|f| f.new_subtask = s),
            Message::AddSubtask => self.edit(|f| {
                let text = std::mem::take(&mut f.new_subtask).trim().to_string();
                if !text.is_empty() {
                    f.subtasks.push(Subtask { done: false, text });
                }
            }),
            Message::EditDayLog(s) => {
                self.day_log = s;
                Action::None
            }
            Message::AddDayLog => {
                let text = std::mem::take(&mut self.day_log).trim().to_string();
                if text.is_empty() {
                    return Action::None;
                }
                Action::AddLog {
                    task_id: None,
                    text,
                }
            }
            Message::DeleteLog(id) => Action::DeleteLog(id),
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
        }
    }

    /// Changes the open form; any edit clears its error.
    fn edit(&mut self, f: impl FnOnce(&mut EditForm)) -> Action {
        if let Some(form) = self.editing.as_mut() {
            f(form);
            form.error = None;
        }
        Action::None
    }

    fn task(&self, id: TaskId) -> Option<&Task> {
        self.data.as_ref()?.tasks.iter().find(|t| t.id == id)
    }

    fn section_of_id(&self, id: TaskId) -> Option<Section> {
        let data = self.data.as_ref()?;
        section_of(self.task(id)?, data.day, data.today, &data.tz)
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

        let overdue = section_tasks(data, Section::Overdue);
        if !overdue.is_empty() {
            let heading = heading_row(
                text("Overdue")
                    .size(theme::TEXT_SMALL)
                    .style(theme::danger)
                    .into(),
                &overdue,
            );
            content = content.push(self.group(heading, overdue, data));
        }

        let day_tasks = section_tasks(data, Section::Day);
        if day_tasks.is_empty() {
            content = content.push(
                column![
                    theme::section_heading(day_heading(data.day, data.today)),
                    text(empty_message(data.day, data.today))
                        .size(theme::TEXT_BODY)
                        .style(theme::muted),
                ]
                .spacing(theme::SPACING / 2.0),
            );
        } else {
            let heading = heading_row(
                theme::section_heading(day_heading(data.day, data.today)).into(),
                &day_tasks,
            );
            content = content.push(self.group(heading, day_tasks, data));
        }

        if let Some(side) = side_work(data, &self.day_log) {
            content = content.push(side);
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

        scrollable(
            container(
                content
                    .padding(theme::SPACING * 2.0)
                    .max_width(theme::CONTENT_WIDTH),
            )
            .center_x(Fill),
        )
        .height(Fill)
        .into()
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
        let header =
            button(text(format!("{arrow}  {label}  {}", tasks.len())).size(theme::TEXT_SMALL))
                .padding([4, 0])
                .style(theme::text_button)
                .on_press(toggle);
        let mut group = column![header].spacing(theme::SPACING / 2.0);
        if open {
            if section == Section::Deleted {
                group = group
                    .push(Column::with_children(tasks.into_iter().map(deleted_row)).spacing(2));
            } else {
                group = group.push(self.rows(tasks, data));
            }
        }
        group.into()
    }

    fn rows<'a>(&'a self, tasks: Vec<&'a Task>, data: &'a DayData) -> Element<'a, Message> {
        Column::with_children(tasks.into_iter().map(|task| match &self.editing {
            Some(form) if form.id == task.id => edit_card(task, form, data.today),
            _ => task_row(task, data.today, self.selected == Some(task.id)),
        }))
        .spacing(6)
        .into()
    }
}

/// A section heading with the task count and, if any task has an estimate,
/// the total (`Today  3 · 4h30m`).
fn heading_row<'a>(label: Element<'a, Message>, tasks: &[&Task]) -> Element<'a, Message> {
    let total: u32 = tasks.iter().filter_map(|t| t.estimate_minutes).sum();
    let mut count = tasks.len().to_string();
    if total > 0 {
        count.push_str(&format!("  ·  {}", parse::format_estimate(total)));
    }
    row![
        label,
        text(count).size(theme::TEXT_SMALL).style(theme::muted)
    ]
    .spacing(theme::SPACING / 2.0)
    .into()
}

/// Side work for the shown day: free-standing log lines as ticked items,
/// or `None` when there is nothing to show (a day other than today with
/// none).
fn side_work<'a>(data: &'a DayData, input: &'a str) -> Option<Element<'a, Message>> {
    let lines: Vec<&LogLine> = data
        .logs
        .iter()
        .filter(|l| l.task_id.is_none() && l.date == data.day)
        .collect();
    let can_add = data.day == data.today;
    if lines.is_empty() && !can_add {
        return None;
    }
    let mut group = column![theme::section_heading("Side work")].spacing(theme::SPACING / 2.0);
    let items = Column::with_children(lines.into_iter().map(|line| {
        row![
            container(text("•").size(theme::TEXT_BODY).style(theme::muted)).center_x(20),
            text(&line.text).size(theme::TEXT_BODY).width(Fill),
            button(text("×").size(theme::TEXT_BODY))
                .padding([0, 8])
                .style(theme::text_button)
                .on_press(Message::DeleteLog(line.id)),
        ]
        .spacing(theme::SPACING)
        .padding([6, 0])
        .align_y(Center)
        .into()
    }));
    group = group.push(items);
    if can_add {
        group = group.push(
            text_input("Log side work", input)
                .on_input(Message::EditDayLog)
                .on_submit(Message::AddDayLog)
                .padding([8, 10])
                .size(theme::TEXT_BODY)
                .style(theme::input),
        );
    }
    Some(group.into())
}

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
                theme::ACCENT.scale_alpha(0.7)
            } else {
                Color::TRANSPARENT
            }),
            ..button::Style::default()
        }
    }
}

/// `!` for Medium, `!!` for High, in the priority colour; `None` for Low.
fn priority_marker<'a>(priority: Priority) -> Option<Element<'a, Message>> {
    let level = priority_level(priority);
    (level > 0).then(|| {
        text("!".repeat(usize::from(level)))
            .size(theme::TEXT_BODY)
            .style(move |theme: &Theme| text::Style {
                color: theme::priority_color(theme, level),
            })
            .into()
    })
}

fn task_row<'a>(task: &'a Task, today: Date, selected: bool) -> Element<'a, Message> {
    let id = task.id;
    let open = task.status.is_open();

    let mut line = Row::new().spacing(theme::SPACING * 0.75).align_y(Center);
    if open && let Some(marker) = priority_marker(task.priority) {
        line = line.push(marker);
    }
    let mut title = text(&task.title).size(theme::TEXT_BODY);
    if !open {
        title = title.style(theme::muted);
    }
    line = line.push(title);
    if let Some(project) = &task.project {
        line = line.push(project_chip(project));
    }
    line = line.push(Space::new().width(Fill));
    let (done, total) = subtask_counts(&task.notes);
    if total > 0 {
        line = line.push(
            text(format!("{done}/{total}"))
                .size(theme::TEXT_SMALL)
                .style(theme::muted),
        );
    }
    if let Some(minutes) = task.estimate_minutes {
        line = line.push(
            text(parse::format_estimate(minutes))
                .size(theme::TEXT_SMALL)
                .style(theme::muted),
        );
    }
    if open && let Some(due) = task.due_on {
        line = line.push(
            text(due_label(due, today))
                .size(theme::TEXT_SMALL)
                .style(theme::due_style(task.is_overdue(today))),
        );
    }

    // The checkbox sits inside the card; it captures its own clicks, so
    // they don't open the form.
    button(
        row![
            checkbox(task.is_done()).on_toggle(move |b| Message::Toggle(id, b)),
            line,
        ]
        .spacing(theme::SPACING * 0.75)
        .align_y(Center),
    )
    .width(Fill)
    .padding([11, 14])
    .style(task_card_style(!open, selected))
    .on_press(Message::Expand(id))
    .into()
}

/// A deleted task: its title, dimmed, with Restore and Delete forever.
fn deleted_row(task: &Task) -> Element<'_, Message> {
    row![
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
    .padding([4, 10])
    .align_y(Center)
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

/// Low, Medium, High as a segmented control.
fn priority_picker<'a>(current: Priority) -> Element<'a, Message> {
    let buttons = PRIORITIES.into_iter().map(|(label, p)| {
        button(text(label).size(theme::TEXT_SMALL))
            .padding(theme::tab_padding())
            .style(theme::tab_button(current == p))
            .on_press(Message::SetPriority(p))
            .into()
    });
    container(Row::with_children(buttons).spacing(2))
        .padding(3)
        .style(theme::tab_bar)
        .into()
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

/// A month calendar for picking the due date, Monday first.
fn calendar<'a>(month: Date, selected: Option<Date>, today: Date) -> Element<'a, Message> {
    const CELL: f32 = 36.0;
    let first = month.first_of_month();
    let offset = i64::from(first.weekday().to_monday_zero_offset());
    let start = first.saturating_sub(offset.days());

    let arrow = |icon, step| {
        button(theme::icon(icon))
            .padding(theme::ICON_PADDING)
            .style(theme::icon_button)
            .on_press(Message::CalendarMonth(step))
    };
    let header = row![
        arrow(theme::Icon::ChevronLeft, -1),
        text(first.strftime("%B %Y").to_string())
            .size(theme::TEXT_BODY)
            .width(Fill)
            .align_x(Center),
        arrow(theme::Icon::ChevronRight, 1),
    ]
    .align_y(Center)
    .width(CELL * 7.0);

    let names = Row::with_children(["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].map(|n| {
        text(n)
            .size(theme::TEXT_SMALL)
            .style(theme::muted)
            .width(CELL)
            .align_x(Center)
            .into()
    }));

    let mut weeks = Column::new().spacing(2);
    for w in 0..6i64 {
        let days = (0..7i64).map(|d| {
            let date = start.saturating_add((w * 7 + d).days());
            let in_month = date.month() == first.month();
            let is_selected = selected == Some(date);
            let is_today = date == today;
            button(
                text(date.day().to_string())
                    .size(theme::TEXT_SMALL)
                    .width(Fill)
                    .align_x(Center),
            )
            .width(CELL)
            .padding([7, 0])
            .style(move |theme: &Theme, status| {
                calendar_day(theme, status, in_month, is_selected, is_today)
            })
            .on_press(Message::PickDue(date))
            .into()
        });
        weeks = weeks.push(Row::with_children(days).spacing(0));
    }

    let mut col = column![header, names, weeks].spacing(6);
    if selected.is_some() {
        col = col.push(
            row![
                Space::new().width(Fill),
                button(text("Clear").size(theme::TEXT_SMALL))
                    .padding([4, 10])
                    .style(theme::text_button)
                    .on_press(Message::ClearDue),
            ]
            .width(CELL * 7.0),
        );
    }
    container(col)
        .padding(theme::SPACING)
        .style(|theme: &Theme| container::Style {
            background: Some(theme.palette().background.into()),
            border: border::rounded(theme::RADIUS)
                .width(1)
                .color(theme::muted_color(theme).scale_alpha(0.25)),
            shadow: iced::Shadow {
                color: Color::BLACK.scale_alpha(0.3),
                offset: iced::Vector::new(0.0, 6.0),
                blur_radius: 18.0,
            },
            ..container::Style::default()
        })
        .into()
}

fn calendar_day(
    theme: &Theme,
    status: button::Status,
    in_month: bool,
    selected: bool,
    today: bool,
) -> button::Style {
    let p = theme.palette();
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let (background, text_color) = if selected {
        (Some(theme::ACCENT.into()), Color::WHITE)
    } else {
        let color = if today {
            theme::ACCENT
        } else if in_month {
            p.text
        } else {
            theme::muted_color(theme).scale_alpha(0.6)
        };
        (hovered.then(|| p.text.scale_alpha(0.08).into()), color)
    };
    button::Style {
        background,
        text_color,
        border: border::rounded(6),
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
        selected_background: theme::ACCENT.into(),
        shadow: iced::Shadow {
            color: Color::BLACK.scale_alpha(0.3),
            offset: iced::Vector::new(0.0, 6.0),
            blur_radius: 18.0,
        },
    }
}

fn edit_card<'a>(task: &'a Task, form: &'a EditForm, today: Date) -> Element<'a, Message> {
    let id = task.id;
    let notes = text_editor(&form.notes)
        .placeholder("Notes")
        .on_action(Message::EditNotes)
        .height(96)
        .padding(10)
        .size(theme::TEXT_BODY)
        .style(theme::editor);

    let mut body = column![
        row![
            checkbox(task.is_done()).on_toggle(move |b| Message::Toggle(id, b)),
            field("Title", &form.title, Message::EditTitle),
        ]
        .spacing(theme::SPACING / 2.0)
        .align_y(Center),
        row![
            labelled("Project", project_field(form)),
            labelled(
                "Due",
                popup::Popup::new(
                    due_button(form.due, today),
                    form.calendar.map(|month| calendar(month, form.due, today)),
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

    body = body.push(labelled("Priority", priority_picker(form.priority)));

    if let Some(error) = &form.error {
        body = body.push(text(error).size(theme::TEXT_SMALL).style(theme::danger));
    }

    body = body.push(labelled("Notes", notes));
    body = body.push(labelled("Subtasks", subtasks_view(form)));

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

/// A widget that shows a pop-up under its anchor, on the overlay layer, so
/// it covers what is below instead of pushing it down. A click outside both
/// the pop-up and the anchor sends `on_dismiss`.
mod popup {
    use iced::advanced::layout::{self, Layout};
    use iced::advanced::overlay;
    use iced::advanced::renderer;
    use iced::advanced::widget::{self, Tree, Widget};
    use iced::advanced::{Clipboard, Shell};
    use iced::{Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector, mouse};

    /// The space between the anchor and the pop-up.
    const GAP: f32 = 6.0;

    pub struct Popup<'a, Message> {
        anchor: Element<'a, Message>,
        /// The pop-up, or a placeholder while closed (so the tree keeps two
        /// children).
        content: Element<'a, Message>,
        open: bool,
        on_dismiss: Message,
    }

    impl<'a, Message: 'a> Popup<'a, Message> {
        pub fn new(
            anchor: impl Into<Element<'a, Message>>,
            content: Option<Element<'a, Message>>,
            on_dismiss: Message,
        ) -> Self {
            let open = content.is_some();
            Popup {
                anchor: anchor.into(),
                content: content.unwrap_or_else(|| iced::widget::space().into()),
                open,
                on_dismiss,
            }
        }
    }

    impl<Message: Clone> Widget<Message, Theme, Renderer> for Popup<'_, Message> {
        fn children(&self) -> Vec<Tree> {
            vec![Tree::new(&self.anchor), Tree::new(&self.content)]
        }

        fn diff(&self, tree: &mut Tree) {
            tree.diff_children(&[&self.anchor, &self.content]);
        }

        fn size(&self) -> Size<Length> {
            self.anchor.as_widget().size()
        }

        fn layout(
            &mut self,
            tree: &mut Tree,
            renderer: &Renderer,
            limits: &layout::Limits,
        ) -> layout::Node {
            self.anchor
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits)
        }

        fn update(
            &mut self,
            tree: &mut Tree,
            event: &Event,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            renderer: &Renderer,
            clipboard: &mut dyn Clipboard,
            shell: &mut Shell<'_, Message>,
            viewport: &Rectangle,
        ) {
            self.anchor.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }

        fn draw(
            &self,
            tree: &Tree,
            renderer: &mut Renderer,
            theme: &Theme,
            style: &renderer::Style,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            viewport: &Rectangle,
        ) {
            self.anchor.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
        }

        fn mouse_interaction(
            &self,
            tree: &Tree,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            viewport: &Rectangle,
            renderer: &Renderer,
        ) -> mouse::Interaction {
            self.anchor.as_widget().mouse_interaction(
                &tree.children[0],
                layout,
                cursor,
                viewport,
                renderer,
            )
        }

        fn operate(
            &mut self,
            tree: &mut Tree,
            layout: Layout<'_>,
            renderer: &Renderer,
            operation: &mut dyn widget::Operation,
        ) {
            self.anchor
                .as_widget_mut()
                .operate(&mut tree.children[0], layout, renderer, operation);
        }

        fn overlay<'b>(
            &'b mut self,
            tree: &'b mut Tree,
            layout: Layout<'b>,
            renderer: &Renderer,
            viewport: &Rectangle,
            translation: Vector,
        ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
            let mut children = tree.children.iter_mut();
            let anchor_tree = children.next()?;
            let content_tree = children.next()?;
            let base = self.anchor.as_widget_mut().overlay(
                anchor_tree,
                layout,
                renderer,
                viewport,
                translation,
            );
            if !self.open {
                return base;
            }
            let anchor = layout.bounds() + translation;
            let popup = overlay::Element::new(Box::new(Overlay {
                content: &mut self.content,
                tree: content_tree,
                anchor,
                on_dismiss: self.on_dismiss.clone(),
            }));
            Some(overlay::Group::with_children(base.into_iter().chain([popup]).collect()).overlay())
        }
    }

    struct Overlay<'a, 'b, Message> {
        content: &'b mut Element<'a, Message>,
        tree: &'b mut Tree,
        /// The anchor's bounds, in window coordinates.
        anchor: Rectangle,
        on_dismiss: Message,
    }

    impl<Message: Clone> overlay::Overlay<Message, Theme, Renderer> for Overlay<'_, '_, Message> {
        fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
            let limits = layout::Limits::new(Size::ZERO, bounds);
            let node = self
                .content
                .as_widget_mut()
                .layout(self.tree, renderer, &limits);
            let size = node.size();
            // Below the anchor, kept inside the window; above it when there
            // is no room below.
            let x = self.anchor.x.min(bounds.width - size.width).max(0.0);
            let below = self.anchor.y + self.anchor.height + GAP;
            let y = if below + size.height <= bounds.height {
                below
            } else {
                (self.anchor.y - GAP - size.height).max(0.0)
            };
            node.move_to(Point::new(x, y))
        }

        fn draw(
            &self,
            renderer: &mut Renderer,
            theme: &Theme,
            style: &renderer::Style,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
        ) {
            self.content.as_widget().draw(
                self.tree,
                renderer,
                theme,
                style,
                layout,
                cursor,
                &layout.bounds(),
            );
        }

        fn update(
            &mut self,
            event: &Event,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            renderer: &Renderer,
            clipboard: &mut dyn Clipboard,
            shell: &mut Shell<'_, Message>,
        ) {
            self.content.as_widget_mut().update(
                self.tree,
                event,
                layout,
                cursor,
                renderer,
                clipboard,
                shell,
                &layout.bounds(),
            );
            if let Event::Mouse(mouse::Event::ButtonPressed(_)) = event {
                if cursor.is_over(layout.bounds()) {
                    // Nothing under the pop-up gets the click.
                    shell.capture_event();
                } else if !cursor.is_over(self.anchor) {
                    shell.publish(self.on_dismiss.clone());
                }
            }
        }

        fn mouse_interaction(
            &self,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            renderer: &Renderer,
        ) -> mouse::Interaction {
            self.content.as_widget().mouse_interaction(
                self.tree,
                layout,
                cursor,
                &layout.bounds(),
                renderer,
            )
        }
    }

    impl<'a, Message: Clone + 'a> From<Popup<'a, Message>> for Element<'a, Message> {
        fn from(popup: Popup<'a, Message>) -> Self {
            Element::new(popup)
        }
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
            // Due last week, done today: completed today.
            done_on(task(10, "late but done", Some(date(2026, 11, 9))), today()),
            planned(task(11, "planned wed", None), date(2026, 11, 18)),
            with_status(task(12, "doing today", Some(today())), TaskStatus::Doing),
        ]
    }

    #[test]
    fn sections_for_today() {
        let data = data_for(today(), mixed());
        assert_eq!(ids(&section_tasks(&data, Section::Overdue)), vec![4, 1]);
        // Doing is a plain open task now.
        assert_eq!(ids(&section_tasks(&data, Section::Day)), vec![3, 2, 12]);
        let mut done = ids(&section_tasks(&data, Section::Done));
        done.sort();
        assert_eq!(done, vec![5, 10]);
        assert_eq!(ids(&section_tasks(&data, Section::NoDate)), vec![7]);
        assert_eq!(ids(&section_tasks(&data, Section::Deleted)), vec![6]);
        for id in [8, 9, 11] {
            let t = data.tasks.iter().find(|t| t.id == id).unwrap();
            assert_eq!(
                section_of(t, today(), today(), &TimeZone::UTC),
                None,
                "{id}"
            );
        }
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
            vec![3, 6, 5, 2, 1]
        );
    }

    #[test]
    fn visible_rows_skip_collapsed_sections_and_deleted() {
        let data = data_for(today(), mixed());
        assert_eq!(visible_rows(&data, false, false), vec![4, 1, 3, 2, 12]);
        let all = visible_rows(&data, true, true);
        assert_eq!(all.len(), 8);
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
        assert_eq!(view.selected, Some(4));
        view.on_key(NavKey::Up);
        assert_eq!(view.selected, Some(4), "stops at the top");
        assert_eq!(
            view.on_key(NavKey::Toggle),
            Action::SetStatus(4, TaskStatus::Done)
        );
        view.on_key(NavKey::Open);
        assert_eq!(view.editing.as_ref().map(|f| f.id), Some(4));
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
    fn side_work_add_and_delete() {
        let mut view = loaded(vec![]);
        assert_eq!(view.update(Message::AddDayLog), Action::None, "empty");
        view.update(Message::EditDayLog("  helped Sam debug the deploy ".into()));
        assert_eq!(
            view.update(Message::AddDayLog),
            Action::AddLog {
                task_id: None,
                text: "helped Sam debug the deploy".into()
            }
        );
        assert!(view.day_log.is_empty());
        assert_eq!(view.update(Message::DeleteLog(42)), Action::DeleteLog(42));
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
