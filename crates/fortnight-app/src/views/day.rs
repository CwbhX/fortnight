//! The Today tab: one day's tasks (PLAN.md 4.2 "Today"). Owner: agent J.
//!
//! Sections, top to bottom:
//!
//! - **Overdue** (only when the shown day is today, and only if any): open
//!   tasks whose deadline has passed.
//! - **The day**: open tasks whose [`Task::day`] is the shown day. On today
//!   this also carries over open tasks planned for an earlier day whose
//!   deadline has not passed.
//! - **Done**, collapsed: done and dropped tasks whose day is the shown day,
//!   dimmed.
//! - **Log**: free-standing log lines dated the shown day, with an input to
//!   add one (today only, see below).
//! - **No date**, collapsed, with a count: open tasks with no day (the
//!   inbox).
//!
//! The quick-add bar and the date navigation live in the top bar.
//!
//! Clicking a row (or `Enter` on the selected row) expands it in place into
//! an edit form, one row at a time: title, notes, project, due date, planned
//! day, estimate, priority, private, status buttons and the task's log.
//! `↑` `↓` move a selection highlight through the visible rows (stopping at
//! the ends), `Space` toggles done and `Escape` collapses the form, then
//! clears the selection.
//!
//! The logic lives in pure functions ([`section_of`], [`section_tasks`],
//! [`visible_rows`], [`EditForm::apply`]) and in [`DayView::update`] /
//! [`DayView::on_key`], so it is tested without a window.
//!
//! Skipped for the prototype: drag ordering, delete confirmation (main.rs
//! has undo), strikethrough on done tasks (iced text has none), the time of
//! day on log lines (the view gets no time zone), and adding a free-standing
//! log line to a day other than today (`Action::AddLog` has no date; main.rs
//! stamps it now, so the input only shows on today).

use std::cmp::Ordering;

use fortnight_core::domain::{EventId, MAX_PRIORITY, Priority, Task, TaskId, TaskStatus};
use fortnight_core::parse;
use iced::widget::{
    Column, Row, Space, button, checkbox, column, container, row, scrollable, text, text_editor,
    text_input,
};
use iced::{Center, Element, Fill, Theme};
use jiff::Timestamp;
use jiff::civil::Date;

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
    EditDue(String),
    EditPlanned(String),
    EditEstimate(String),
    EditNotes(text_editor::Action),
    SetPriority(Priority),
    SetPrivate(bool),
    /// A status button in the open form (Start, Stop, Drop, Reopen).
    SetStatus(TaskStatus),
    /// The open form's log input.
    EditTaskLog(String),
    AddTaskLog,
    /// The free-standing log input.
    EditDayLog(String),
    AddDayLog,
    DeleteLog(EventId),
    Save,
    Delete,
    Close,
    /// Show or hide the Done section.
    ToggleShowDone,
    /// Show or hide the No date section.
    ToggleShowNoDate,
}

/// What `main.rs` must do. FROZEN contract (round 2).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Set a task's status (checkbox: `Done`/`Todo`; also `Doing` for in
    /// progress and `Dropped` for won't do).
    SetStatus(TaskId, TaskStatus),
    /// Save the editable fields (title, notes, project, due_on, planned_for,
    /// estimate_minutes, priority, private). `main.rs` records it for undo.
    Update(Task),
    /// Delete a task. `main.rs` keeps it for undo (Ctrl/Cmd+Z).
    Delete(TaskId),
    /// Add a log line: attached to a task, or free-standing (`None`).
    /// `main.rs` stamps it with the current time.
    AddLog {
        task_id: Option<TaskId>,
        text: String,
    },
    DeleteLog(EventId),
}

/// A log line, as the view needs it. FROZEN contract (round 2).
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

/// Everything the view shows, supplied by `main.rs`. FROZEN contract
/// (round 2).
#[derive(Debug, Clone, PartialEq)]
pub struct DayData {
    /// The day shown (the shared selected date).
    pub day: Date,
    /// The local date today. Overdue is relative to this.
    pub today: Date,
    /// Every task, done or not. The view picks what to show using
    /// `Task::day()` and `Task::is_overdue(today)`.
    pub tasks: Vec<Task>,
    /// Every log line, oldest first.
    pub logs: Vec<LogLine>,
}

#[derive(Debug, Default)]
pub struct DayView {
    data: Option<DayData>,
    show_done: bool,
    show_no_date: bool,
    /// The keyboard selection highlight.
    selected: Option<TaskId>,
    /// The expanded row's form.
    editing: Option<EditForm>,
    /// The free-standing log input.
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
}

impl Section {
    /// Display order.
    pub const ALL: [Section; 4] = [
        Section::Overdue,
        Section::Day,
        Section::Done,
        Section::NoDate,
    ];
}

/// The section `task` belongs in when `day` is shown, or `None` if it is not
/// on screen. See the module doc for the rules.
pub fn section_of(task: &Task, day: Date, today: Date) -> Option<Section> {
    let shows_today = day == today;
    if !task.status.is_open() {
        return (task.day() == Some(day)).then_some(Section::Done);
    }
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

/// Order for open tasks: in progress first, then higher priority, then
/// earlier day (carried-over before today's), then manual position, then id.
fn open_order(a: &Task, b: &Task) -> Ordering {
    let doing = |t: &Task| t.status != TaskStatus::Doing;
    doing(a)
        .cmp(&doing(b))
        .then(b.priority.cmp(&a.priority))
        .then(a.day().cmp(&b.day()))
        .then(a.position.total_cmp(&b.position))
        .then(a.id.cmp(&b.id))
}

/// The tasks in `section`, sorted. Overdue: oldest deadline first, then
/// [`open_order`]. The day and No date: [`open_order`]. Done: most recently
/// finished first (dropped tasks by when they were last changed).
pub fn section_tasks(data: &DayData, section: Section) -> Vec<&Task> {
    let mut out: Vec<&Task> = data
        .tasks
        .iter()
        .filter(|t| section_of(t, data.day, data.today) == Some(section))
        .collect();
    match section {
        Section::Overdue => out.sort_by(|a, b| a.due_on.cmp(&b.due_on).then(open_order(a, b))),
        Section::Day | Section::NoDate => out.sort_by(|a, b| open_order(a, b)),
        Section::Done => out.sort_by(|a, b| {
            let finished = |t: &Task| t.completed_at.unwrap_or(t.updated_at);
            finished(b).cmp(&finished(a)).then(a.id.cmp(&b.id))
        }),
    }
    out
}

/// The rows on screen, top to bottom, skipping collapsed sections.
pub fn visible_rows(data: &DayData, show_done: bool, show_no_date: bool) -> Vec<TaskId> {
    Section::ALL
        .into_iter()
        .filter(|s| match s {
            Section::Done => show_done,
            Section::NoDate => show_no_date,
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

/// The in-place edit form for one task.
#[derive(Debug, Clone)]
pub struct EditForm {
    pub id: TaskId,
    pub title: String,
    pub project: String,
    pub due: String,
    /// The due field as first shown, so an untouched field is not re-parsed.
    pub original_due: String,
    pub planned: String,
    pub original_planned: String,
    pub estimate: String,
    pub original_estimate: String,
    pub priority: Priority,
    pub private: bool,
    pub notes: text_editor::Content,
    /// The log input under the task's log lines.
    pub log: String,
    pub error: Option<String>,
}

/// Reads a date field: empty clears, unchanged keeps `current` without
/// parsing, anything else goes through `parse_date`.
fn read_date(
    label: &str,
    input: &str,
    original: &str,
    current: Option<Date>,
    today: Date,
    parse_date: &impl Fn(&str, Date) -> Option<Date>,
) -> Result<Option<Date>, String> {
    let input = input.trim();
    if input.is_empty() {
        Ok(None)
    } else if input == original.trim() {
        Ok(current)
    } else {
        parse_date(input, today).map(Some).ok_or_else(|| {
            format!(
                "Can't read \"{input}\" as a {} date. Try fri, tomorrow, nov 20 or +3d.",
                label.to_lowercase()
            )
        })
    }
}

impl EditForm {
    pub fn from_task(task: &Task) -> Self {
        let date = |d: Option<Date>| d.map(|d| d.to_string()).unwrap_or_default();
        let estimate = task
            .estimate_minutes
            .map(parse::format_estimate)
            .unwrap_or_default();
        Self {
            id: task.id,
            title: task.title.clone(),
            project: task.project.clone().unwrap_or_default(),
            due: date(task.due_on),
            original_due: date(task.due_on),
            planned: date(task.planned_for),
            original_planned: date(task.planned_for),
            estimate: estimate.clone(),
            original_estimate: estimate,
            priority: task.priority,
            private: task.private,
            notes: text_editor::Content::with_text(&task.notes),
            log: String::new(),
            error: None,
        }
    }

    /// Builds the updated task from the form, or an error message.
    ///
    /// Date and estimate fields: empty clears; unchanged keeps the task's
    /// value without parsing; anything else is parsed (`parse_date`,
    /// [`parse::parse_estimate`]).
    pub fn apply(
        &self,
        task: &Task,
        today: Date,
        parse_date: impl Fn(&str, Date) -> Option<Date>,
    ) -> Result<Task, String> {
        let title = self.title.trim();
        if title.is_empty() {
            return Err("The title can't be empty.".to_string());
        }
        let due_on = read_date(
            "Due",
            &self.due,
            &self.original_due,
            task.due_on,
            today,
            &parse_date,
        )?;
        let planned_for = read_date(
            "Planned",
            &self.planned,
            &self.original_planned,
            task.planned_for,
            today,
            &parse_date,
        )?;
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
        let project = self.project.trim().trim_start_matches('#').trim();
        Ok(Task {
            title: title.to_string(),
            project: (!project.is_empty()).then(|| project.to_string()),
            due_on,
            planned_for,
            estimate_minutes,
            priority: self.priority.min(MAX_PRIORITY),
            private: self.private,
            notes: self.notes.text().trim_end().to_string(),
            ..task.clone()
        })
    }
}

impl DayView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces what the view shows. `main.rs` calls this after EVERY
    /// message (and on the minute tick), so it must be cheap: if `data`
    /// equals what the view already has, return early. It must keep
    /// in-progress UI state (an open edit form, typed text, the selection)
    /// unless the thing it belongs to is gone.
    ///
    /// The open form closes when its task leaves the screen; if it moved
    /// into a collapsed section (e.g. Drop), that section opens. When the
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
                None => self.editing = None,
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
                {
                    self.editing = Some(EditForm::from_task(task));
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
                    self.editing = Some(EditForm::from_task(task));
                    self.selected = Some(id);
                }
                Action::None
            }
            Message::EditTitle(s) => self.edit(|f| f.title = s),
            Message::EditProject(s) => self.edit(|f| f.project = s),
            Message::EditDue(s) => self.edit(|f| f.due = s),
            Message::EditPlanned(s) => self.edit(|f| f.planned = s),
            Message::EditEstimate(s) => self.edit(|f| f.estimate = s),
            Message::EditNotes(action) => self.edit(|f| f.notes.perform(action)),
            Message::SetPriority(p) => self.edit(|f| f.priority = p.min(MAX_PRIORITY)),
            Message::SetPrivate(b) => self.edit(|f| f.private = b),
            Message::EditTaskLog(s) => self.edit(|f| f.log = s),
            Message::SetStatus(status) => match &self.editing {
                Some(form) => Action::SetStatus(form.id, status),
                None => Action::None,
            },
            Message::AddTaskLog => {
                let Some(form) = self.editing.as_mut() else {
                    return Action::None;
                };
                let text = std::mem::take(&mut form.log).trim().to_string();
                if text.is_empty() {
                    return Action::None;
                }
                Action::AddLog {
                    task_id: Some(form.id),
                    text,
                }
            }
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
                match form.apply(task, data.today, parse::parse_date) {
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
                Some(form) => Action::Delete(form.id),
                None => Action::None,
            },
            Message::Close => {
                self.editing = None;
                Action::None
            }
            Message::ToggleShowDone => {
                self.show_done = !self.show_done;
                Action::None
            }
            Message::ToggleShowNoDate => {
                self.show_no_date = !self.show_no_date;
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
        section_of(self.task(id)?, data.day, data.today)
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

        let done = section_tasks(data, Section::Done);
        if !done.is_empty() {
            content = content.push(self.collapsible(
                "Done",
                self.show_done,
                Message::ToggleShowDone,
                done,
                data,
            ));
        }

        if let Some(log) = day_log(data, &self.day_log) {
            content = content.push(log);
        }

        let no_date = section_tasks(data, Section::NoDate);
        if !no_date.is_empty() {
            content = content.push(self.collapsible(
                "No date",
                self.show_no_date,
                Message::ToggleShowNoDate,
                no_date,
                data,
            ));
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
            group = group.push(self.rows(tasks, data));
        }
        group.into()
    }

    fn rows<'a>(&'a self, tasks: Vec<&'a Task>, data: &'a DayData) -> Element<'a, Message> {
        Column::with_children(tasks.into_iter().map(|task| match &self.editing {
            Some(form) if form.id == task.id => edit_card(task, form, &data.logs),
            _ => task_row(task, data.today, self.selected == Some(task.id)),
        }))
        .spacing(2)
        .into()
    }
}

/// A section heading with the open-task count and, if any task has an
/// estimate, the total (`Today  3 · 4h30m`).
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

/// The free-standing log for the shown day, or `None` when there is nothing
/// to show (a day other than today with no lines).
fn day_log<'a>(data: &'a DayData, input: &'a str) -> Option<Element<'a, Message>> {
    let lines: Vec<&LogLine> = data
        .logs
        .iter()
        .filter(|l| l.task_id.is_none() && l.date == data.day)
        .collect();
    let can_add = data.day == data.today;
    if lines.is_empty() && !can_add {
        return None;
    }
    let mut group = column![theme::section_heading("Log")].spacing(theme::SPACING / 2.0);
    group = group.push(log_lines(lines));
    if can_add {
        group = group.push(
            text_input("Log work that isn't a task", input)
                .on_input(Message::EditDayLog)
                .on_submit(Message::AddDayLog)
                .padding([8, 10])
                .size(theme::TEXT_SMALL)
                .style(theme::input),
        );
    }
    Some(group.into())
}

/// Log lines as quiet bullets with a delete button each.
fn log_lines<'a>(lines: Vec<&'a LogLine>) -> Element<'a, Message> {
    Column::with_children(lines.into_iter().map(|line| {
        row![
            text("·").size(theme::TEXT_BODY).style(theme::muted),
            text(&line.text).size(theme::TEXT_BODY).width(Fill),
            button(text("×").size(theme::TEXT_BODY))
                .padding([0, 8])
                .style(theme::text_button)
                .on_press(Message::DeleteLog(line.id)),
        ]
        .spacing(theme::SPACING / 2.0)
        .align_y(Center)
        .into()
    }))
    .spacing(4)
    .into()
}

/// A selected row: the hovered look of [`theme::row_button`], always.
fn selected_row(theme: &Theme, _status: button::Status) -> button::Style {
    theme::row_button(theme, button::Status::Hovered)
}

/// `!`, `!!` or `!!!` in the priority colour; `None` for normal priority.
fn priority_marker<'a>(priority: Priority) -> Option<Element<'a, Message>> {
    (priority > 0).then(|| {
        text("!".repeat(usize::from(priority.min(MAX_PRIORITY))))
            .size(theme::TEXT_BODY)
            .style(move |theme: &Theme| text::Style {
                color: theme::priority_color(theme, priority),
            })
            .into()
    })
}

fn private_marker<'a>() -> Element<'a, Message> {
    theme::icon(theme::Icon::Lock).width(13).height(13).into()
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
    if task.private {
        line = line.push(private_marker());
    }
    line = line.push(Space::new().width(Fill));
    match task.status {
        TaskStatus::Doing => {
            line = line.push(
                text("In progress")
                    .size(theme::TEXT_SMALL)
                    .style(theme::accent),
            );
        }
        TaskStatus::Dropped => {
            line = line.push(text("Dropped").size(theme::TEXT_SMALL).style(theme::muted));
        }
        TaskStatus::Todo | TaskStatus::Done => {}
    }
    if let Some(minutes) = task.estimate_minutes {
        line = line.push(
            text(parse::format_estimate(minutes))
                .size(theme::TEXT_SMALL)
                .style(theme::muted),
        );
    }
    if let Some(due) = task.due_on {
        line = line.push(
            text(due_label(due, today))
                .size(theme::TEXT_SMALL)
                .style(theme::due_style(task.is_overdue(today))),
        );
    }

    row![
        checkbox(task.is_done()).on_toggle(move |b| Message::Toggle(id, b)),
        button(line)
            .width(Fill)
            .padding([9, 10])
            .style(if selected {
                selected_row
            } else {
                theme::row_button
            })
            .on_press(Message::Expand(id)),
    ]
    .spacing(theme::SPACING / 2.0)
    .align_y(Center)
    .into()
}

fn project_chip(project: &str) -> Element<'_, Message> {
    container(text(format!("#{project}")).size(theme::TEXT_SMALL))
        .padding(theme::chip_padding())
        .style(theme::chip)
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

/// Normal, `!`, `!!`, `!!!` as a segmented control.
fn priority_picker<'a>(current: Priority) -> Element<'a, Message> {
    let buttons = (0..=MAX_PRIORITY).map(|p| {
        let label = if p == 0 {
            "Normal".to_string()
        } else {
            "!".repeat(usize::from(p))
        };
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

/// Start / Stop / Drop / Reopen, depending on the status.
fn status_buttons<'a>(status: TaskStatus) -> Element<'a, Message> {
    let small = |label: &'a str, to: TaskStatus| {
        button(text(label).size(theme::TEXT_SMALL))
            .padding([5, 12])
            .style(theme::secondary_button)
            .on_press(Message::SetStatus(to))
    };
    let drop = || {
        button(text("Drop").size(theme::TEXT_SMALL))
            .padding([5, 12])
            .style(theme::text_button)
            .on_press(Message::SetStatus(TaskStatus::Dropped))
    };
    let buttons: Row<'a, Message> = match status {
        TaskStatus::Todo => row![small("Start", TaskStatus::Doing), drop()],
        TaskStatus::Doing => row![
            text("In progress")
                .size(theme::TEXT_SMALL)
                .style(theme::accent),
            small("Stop", TaskStatus::Todo),
            drop()
        ],
        TaskStatus::Done => row![
            text("Done").size(theme::TEXT_SMALL).style(theme::muted),
            small("Reopen", TaskStatus::Todo)
        ],
        TaskStatus::Dropped => row![
            text("Dropped").size(theme::TEXT_SMALL).style(theme::muted),
            small("Reopen", TaskStatus::Todo)
        ],
    };
    buttons.spacing(theme::SPACING / 2.0).align_y(Center).into()
}

fn edit_card<'a>(task: &'a Task, form: &'a EditForm, logs: &'a [LogLine]) -> Element<'a, Message> {
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
            labelled(
                "Project",
                field("none", &form.project, Message::EditProject)
            ),
            labelled(
                "Due",
                field("fri, nov 20 or +3d", &form.due, Message::EditDue)
            ),
            labelled(
                "Planned",
                field("same as due", &form.planned, Message::EditPlanned)
            ),
            labelled(
                "Estimate",
                field("2h, 30m or 1h30m", &form.estimate, Message::EditEstimate)
            ),
        ]
        .spacing(theme::SPACING),
        row![
            priority_picker(form.priority),
            checkbox(form.private)
                .label("Private")
                .text_size(theme::TEXT_SMALL)
                .on_toggle(Message::SetPrivate),
            Space::new().width(Fill),
            status_buttons(task.status),
        ]
        .spacing(theme::SPACING)
        .align_y(Center),
    ]
    .spacing(theme::SPACING);

    if let Some(error) = &form.error {
        body = body.push(text(error).size(theme::TEXT_SMALL).style(theme::danger));
    }

    body = body.push(labelled("Notes", notes));

    let lines: Vec<&LogLine> = logs.iter().filter(|l| l.task_id == Some(id)).collect();
    body = body.push(labelled(
        "Log",
        column![
            log_lines(lines),
            text_input("Add a log line", &form.log)
                .on_input(Message::EditTaskLog)
                .on_submit(Message::AddTaskLog)
                .padding([8, 10])
                .size(theme::TEXT_SMALL)
                .style(theme::input),
        ]
        .spacing(theme::SPACING / 2.0),
    ));

    body = body.push(
        row![
            button(text("Save").size(theme::TEXT_BODY))
                .padding([6, 16])
                .style(theme::primary_button)
                .on_press(Message::Save),
            button(text("Close").size(theme::TEXT_BODY))
                .padding([6, 12])
                .style(theme::text_button)
                .on_press(Message::Close),
            Space::new().width(Fill),
            button(text("Delete").size(theme::TEXT_BODY))
                .padding([6, 12])
                .style(theme::danger_button)
                .on_press(Message::Delete),
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

    fn task(id: TaskId, title: &str, due: Option<Date>) -> Task {
        Task {
            id,
            title: title.to_string(),
            due_on: due,
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

    fn data_for(day: Date, tasks: Vec<Task>) -> DayData {
        DayData {
            day,
            today: today(),
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

    fn log(id: EventId, task_id: Option<TaskId>, day: Date, text: &str) -> LogLine {
        LogLine {
            id,
            task_id,
            at: Timestamp::UNIX_EPOCH,
            date: day,
            text: text.to_string(),
        }
    }

    fn no_parse(_: &str, _: Date) -> Option<Date> {
        panic!("parse_date must not be called")
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
            with_status(task(5, "done today", Some(today())), TaskStatus::Done),
            with_status(task(6, "dropped today", Some(today())), TaskStatus::Dropped),
            task(7, "inbox", None),
            task(8, "wednesday", Some(date(2026, 11, 18))),
            with_status(
                task(9, "done wed", Some(date(2026, 11, 18))),
                TaskStatus::Done,
            ),
            with_status(task(10, "done no date", None), TaskStatus::Done),
            planned(task(11, "planned wed", None), date(2026, 11, 18)),
            with_status(task(12, "doing today", Some(today())), TaskStatus::Doing),
        ]
    }

    #[test]
    fn sections_for_today() {
        let data = data_for(today(), mixed());
        assert_eq!(ids(&section_tasks(&data, Section::Overdue)), vec![4, 1]);
        assert_eq!(ids(&section_tasks(&data, Section::Day)), vec![12, 3, 2]);
        let mut done = ids(&section_tasks(&data, Section::Done));
        done.sort();
        assert_eq!(done, vec![5, 6]);
        assert_eq!(ids(&section_tasks(&data, Section::NoDate)), vec![7]);
        // Future-day tasks and done-without-a-day are not shown.
        for id in [8, 9, 10, 11] {
            let t = data.tasks.iter().find(|t| t.id == id).unwrap();
            assert_eq!(section_of(t, today(), today()), None, "{id}");
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

        // A past day: its overdue task is in the day, nothing carries over.
        let fri = date(2026, 11, 13);
        let data = data_for(fri, mixed());
        assert!(section_tasks(&data, Section::Overdue).is_empty());
        assert_eq!(ids(&section_tasks(&data, Section::Day)), vec![1, 3]);
    }

    #[test]
    fn sort_order() {
        let mut a = task(1, "a", Some(today()));
        a.position = 5.0;
        let mut b = task(2, "b", Some(today()));
        b.position = 1.0;
        let mut c = task(3, "c", Some(today()));
        c.priority = 2;
        let d = with_status(task(4, "d", Some(today())), TaskStatus::Doing);
        let e = planned(task(5, "e", None), date(2026, 11, 12));
        let mut f = task(6, "f", Some(today()));
        f.priority = 1;
        let data = data_for(today(), vec![a, b, c, d, e, f]);
        // Doing, then priority, then earlier day, then position.
        assert_eq!(
            ids(&section_tasks(&data, Section::Day)),
            vec![4, 3, 6, 5, 2, 1]
        );

        // Overdue: oldest deadline first.
        let data = data_for(
            today(),
            vec![
                task(1, "a", Some(date(2026, 11, 14))),
                task(2, "b", Some(date(2026, 11, 2))),
            ],
        );
        assert_eq!(ids(&section_tasks(&data, Section::Overdue)), vec![2, 1]);

        // Done: most recently finished first.
        let mut x = with_status(task(1, "x", Some(today())), TaskStatus::Done);
        x.completed_at = Some(Timestamp::from_second(100).unwrap());
        let mut y = with_status(task(2, "y", Some(today())), TaskStatus::Done);
        y.completed_at = Some(Timestamp::from_second(200).unwrap());
        let mut z = with_status(task(3, "z", Some(today())), TaskStatus::Dropped);
        z.updated_at = Timestamp::from_second(150).unwrap();
        let data = data_for(today(), vec![x, y, z]);
        assert_eq!(ids(&section_tasks(&data, Section::Done)), vec![2, 3, 1]);
    }

    #[test]
    fn visible_rows_skip_collapsed_sections() {
        let data = data_for(today(), mixed());
        assert_eq!(visible_rows(&data, false, false), vec![4, 1, 12, 3, 2]);
        let all = visible_rows(&data, true, true);
        assert_eq!(all.len(), 8);
        assert_eq!(all.last(), Some(&7));
    }

    #[test]
    fn due_labels() {
        let cases = [
            (date(2026, 11, 13), "3d ago"),
            (date(2026, 11, 15), "Yesterday"),
            (date(2026, 11, 16), "Today"),
            (date(2026, 11, 17), "Tomorrow"),
            (date(2026, 11, 18), "Wed"),
            (date(2026, 11, 22), "Sun"),
            (date(2026, 11, 23), "in 7d"),
            (date(2026, 11, 29), "in 13d"),
            (date(2026, 11, 30), "Nov 30"),
            (date(2027, 1, 4), "Jan 4, 2027"),
        ];
        for (due, want) in cases {
            assert_eq!(due_label(due, today()), want, "{due}");
        }
    }

    #[test]
    fn headings_and_empty_states() {
        assert_eq!(day_heading(today(), today()), "Today");
        assert_eq!(day_heading(date(2026, 11, 17), today()), "Tomorrow");
        assert_eq!(day_heading(date(2026, 11, 15), today()), "Yesterday");
        assert_eq!(day_heading(date(2026, 11, 18), today()), "Wed 18 Nov");
        assert_eq!(
            empty_message(date(2026, 11, 18), today()),
            "Nothing planned for Wed 18 Nov"
        );
        assert!(empty_message(today(), today()).starts_with("Nothing planned for today"));
    }

    #[test]
    fn keyboard_moves_and_stops_at_ends() {
        let mut view = loaded(mixed());
        // Rows: 4, 1, 12, 3, 2.
        view.on_key(NavKey::Down);
        assert_eq!(view.selected, Some(4));
        view.on_key(NavKey::Up);
        assert_eq!(view.selected, Some(4), "stops at the top");
        for _ in 0..10 {
            view.on_key(NavKey::Down);
        }
        assert_eq!(view.selected, Some(2), "stops at the bottom");
        view.on_key(NavKey::Up);
        assert_eq!(view.selected, Some(3));

        // Up with nothing selected starts at the bottom.
        view.on_key(NavKey::Close);
        assert_eq!(view.selected, None);
        view.on_key(NavKey::Up);
        assert_eq!(view.selected, Some(2));

        // Expanding No date adds its rows at the end.
        view.update(Message::ToggleShowNoDate);
        view.on_key(NavKey::Down);
        assert_eq!(view.selected, Some(7));

        // No rows: nothing selected.
        let mut empty = loaded(Vec::new());
        empty.on_key(NavKey::Down);
        assert_eq!(empty.selected, None);
        assert_eq!(empty.on_key(NavKey::Toggle), Action::None);
    }

    #[test]
    fn keyboard_toggle_open_close() {
        let mut view = loaded(mixed());
        assert_eq!(view.on_key(NavKey::Toggle), Action::None, "no selection");
        view.on_key(NavKey::Down);
        assert_eq!(
            view.on_key(NavKey::Toggle),
            Action::SetStatus(4, TaskStatus::Done)
        );
        view.on_key(NavKey::Open);
        assert_eq!(view.editing.as_ref().map(|f| f.id), Some(4));
        // Enter again keeps typed text.
        view.update(Message::EditTitle("typed".into()));
        view.on_key(NavKey::Open);
        assert_eq!(view.editing.as_ref().unwrap().title, "typed");
        // Escape closes the form, then clears the selection.
        view.on_key(NavKey::Close);
        assert!(view.editing.is_none());
        assert_eq!(view.selected, Some(4));
        view.on_key(NavKey::Close);
        assert_eq!(view.selected, None);

        // Toggling a done row reopens it.
        view.update(Message::ToggleShowDone);
        view.selected = Some(5);
        assert_eq!(
            view.on_key(NavKey::Toggle),
            Action::SetStatus(5, TaskStatus::Todo)
        );
    }

    #[test]
    fn selection_survives_load() {
        let mut view = loaded(mixed());
        view.selected = Some(12);
        // Unrelated change: selection kept.
        let mut tasks = mixed();
        tasks[0].title = "renamed".into();
        view.load(data_for(today(), tasks.clone()));
        assert_eq!(view.selected, Some(12));

        // The selected task is done (moves to the collapsed Done): the next
        // row below is selected.
        tasks.iter_mut().find(|t| t.id == 12).unwrap().status = TaskStatus::Done;
        view.load(data_for(today(), tasks.clone()));
        assert_eq!(view.selected, Some(3));

        // The last row disappears: the one above is selected.
        view.selected = Some(2);
        tasks.retain(|t| t.id != 2);
        view.load(data_for(today(), tasks.clone()));
        assert_eq!(view.selected, Some(3));

        // Paging to a day where none of the rows show: no selection.
        view.load(data_for(date(2026, 11, 18), tasks));
        assert_eq!(view.selected, None);
    }

    #[test]
    fn load_keeps_form_and_follows_it_into_collapsed_sections() {
        let mut view = loaded(mixed());
        view.update(Message::Expand(2));
        view.update(Message::EditTitle("typed".into()));
        view.update(Message::EditDayLog("half a log".into()));
        let mut tasks = mixed();
        tasks.iter_mut().find(|t| t.id == 2).unwrap().status = TaskStatus::Dropped;
        view.load(data_for(today(), tasks.clone()));
        assert_eq!(view.editing.as_ref().unwrap().title, "typed");
        assert!(view.show_done, "Done opens to keep the form on screen");
        assert_eq!(view.day_log, "half a log");

        // Gone from the screen: the form closes.
        view.load(data_for(date(2026, 11, 18), tasks));
        assert!(view.editing.is_none());
    }

    #[test]
    fn toggling_returns_set_status() {
        let mut view = loaded(vec![task(7, "t", None)]);
        assert_eq!(
            view.update(Message::Toggle(7, true)),
            Action::SetStatus(7, TaskStatus::Done)
        );
        assert_eq!(
            view.update(Message::Toggle(7, false)),
            Action::SetStatus(7, TaskStatus::Todo)
        );
    }

    #[test]
    fn status_buttons_act_on_the_open_task() {
        let mut view = loaded(vec![task(7, "t", Some(today()))]);
        assert_eq!(
            view.update(Message::SetStatus(TaskStatus::Doing)),
            Action::None
        );
        view.update(Message::Expand(7));
        for status in [TaskStatus::Doing, TaskStatus::Dropped, TaskStatus::Todo] {
            assert_eq!(
                view.update(Message::SetStatus(status)),
                Action::SetStatus(7, status)
            );
        }
        assert!(view.editing.is_some(), "status buttons keep the form open");
    }

    #[test]
    fn delete_returns_delete_and_closes() {
        let mut view = loaded(vec![task(3, "t", None)]);
        view.update(Message::Expand(3));
        assert_eq!(view.selected, Some(3));
        assert_eq!(view.update(Message::Delete), Action::Delete(3));
        assert!(view.editing.is_none());
    }

    #[test]
    fn only_one_row_expanded_and_click_again_collapses() {
        let mut view = loaded(vec![task(1, "a", None), task(2, "b", None)]);
        view.update(Message::Expand(1));
        view.update(Message::Expand(2));
        assert_eq!(view.editing.as_ref().map(|f| f.id), Some(2));
        view.update(Message::Expand(2));
        assert!(view.editing.is_none());
    }

    #[test]
    fn unchanged_save_returns_none() {
        let mut t = task(4, "Same", Some(date(2026, 11, 20)));
        t.planned_for = Some(today());
        t.estimate_minutes = Some(90);
        t.priority = 2;
        t.private = true;
        t.notes = "notes".into();
        t.project = Some("work".into());
        let mut view = loaded(vec![t]);
        view.update(Message::Expand(4));
        assert_eq!(view.update(Message::Save), Action::None);
        assert!(view.editing.is_none());
    }

    #[test]
    fn edit_form_round_trip_every_field() {
        let t = task(4, "Old", Some(date(2026, 11, 20)));
        let mut view = loaded(vec![t.clone()]);
        view.update(Message::Expand(4));
        view.update(Message::EditTitle("  New title ".into()));
        view.update(Message::EditProject("#home".into()));
        view.update(Message::EditDue("2026-11-27".into()));
        view.update(Message::EditPlanned("tomorrow".into()));
        view.update(Message::EditEstimate("~1h30m".into()));
        view.update(Message::SetPriority(9));
        view.update(Message::SetPrivate(true));
        view.update(Message::EditNotes(text_editor::Action::Edit(
            text_editor::Edit::Paste(std::sync::Arc::new("line one\n".to_string())),
        )));
        let Action::Update(updated) = view.update(Message::Save) else {
            panic!("expected Update");
        };
        assert_eq!(
            updated,
            Task {
                title: "New title".into(),
                project: Some("home".into()),
                due_on: Some(date(2026, 11, 27)),
                planned_for: Some(date(2026, 11, 17)),
                estimate_minutes: Some(90),
                priority: MAX_PRIORITY,
                private: true,
                notes: "line one".into(),
                ..t
            }
        );

        // And back: the form shows the saved values; clearing works.
        let mut form = EditForm::from_task(&updated);
        assert_eq!(form.estimate, "1h30m");
        assert_eq!(form.planned, "2026-11-17");
        assert_eq!(form.priority, MAX_PRIORITY);
        assert!(form.private);
        assert_eq!(form.apply(&updated, today(), no_parse).unwrap(), updated);
        form.estimate.clear();
        form.planned = " ".into();
        form.due.clear();
        let cleared = form.apply(&updated, today(), no_parse).unwrap();
        assert_eq!(cleared.estimate_minutes, None);
        assert_eq!(cleared.planned_for, None);
        assert_eq!(cleared.due_on, None);
    }

    #[test]
    fn apply_edit_rules() {
        let t = task(1, "Title", Some(date(2026, 11, 20)));
        let mut form = EditForm::from_task(&t);

        // Empty due clears the date without parsing.
        form.due = "  ".into();
        form.project = " #work ".into();
        let out = form.apply(&t, today(), no_parse).unwrap();
        assert_eq!(out.due_on, None);
        assert_eq!(out.project.as_deref(), Some("work"));

        // A changed due is parsed; an invalid one is an error.
        form.due = "fri".into();
        let out = form
            .apply(&t, today(), |s, _| (s == "fri").then(|| date(2026, 11, 20)))
            .unwrap();
        assert_eq!(out.due_on, Some(date(2026, 11, 20)));
        form.due = "someday".into();
        assert!(form.apply(&t, today(), |_, _| None).is_err());

        // Empty title is an error; empty project is None.
        form.due = form.original_due.clone();
        form.project.clear();
        assert_eq!(form.apply(&t, today(), no_parse).unwrap().project, None);
        form.title = " ".into();
        assert!(form.apply(&t, today(), no_parse).is_err());
    }

    #[test]
    fn invalid_date_or_estimate_shows_error_and_saves_nothing() {
        let mut view = loaded(vec![task(1, "Title", None)]);
        view.update(Message::Expand(1));

        view.update(Message::EditPlanned("blah".into()));
        assert_eq!(view.update(Message::Save), Action::None);
        let err = view.editing.as_ref().unwrap().error.clone().unwrap();
        assert!(
            err.contains("planned date") && err.contains("blah"),
            "{err}"
        );

        // Editing clears the error.
        view.update(Message::EditPlanned(String::new()));
        assert!(view.editing.as_ref().unwrap().error.is_none());

        view.update(Message::EditDue("someday".into()));
        assert_eq!(view.update(Message::Save), Action::None);
        let err = view.editing.as_ref().unwrap().error.clone().unwrap();
        assert!(err.contains("due date") && err.contains("someday"), "{err}");
        view.update(Message::EditDue(String::new()));

        view.update(Message::EditEstimate("lots".into()));
        assert_eq!(view.update(Message::Save), Action::None);
        let err = view.editing.as_ref().unwrap().error.clone().unwrap();
        assert!(err.contains("estimate") && err.contains("lots"), "{err}");
        assert!(view.editing.is_some(), "the form stays open");
    }

    #[test]
    fn unchanged_estimate_is_not_reparsed() {
        // 0 minutes formats as "0m", which does not parse back; leaving it
        // untouched must keep it rather than error.
        let mut t = task(1, "t", None);
        t.estimate_minutes = Some(0);
        let form = EditForm::from_task(&t);
        assert_eq!(form.apply(&t, today(), no_parse).unwrap(), t);
    }

    #[test]
    fn log_lines_add_and_delete() {
        let mut view = loaded(vec![task(1, "t", Some(today()))]);
        // Free-standing.
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

        // Attached to the open task.
        assert_eq!(view.update(Message::AddTaskLog), Action::None, "no form");
        view.update(Message::Expand(1));
        view.update(Message::EditTaskLog("wrote the intro".into()));
        assert_eq!(
            view.update(Message::AddTaskLog),
            Action::AddLog {
                task_id: Some(1),
                text: "wrote the intro".into()
            }
        );
        assert!(view.editing.as_ref().unwrap().log.is_empty());
        view.update(Message::EditTaskLog("   ".into()));
        assert_eq!(view.update(Message::AddTaskLog), Action::None);

        assert_eq!(view.update(Message::DeleteLog(42)), Action::DeleteLog(42));
    }

    #[test]
    fn view_builds_in_every_state() {
        // No window: just make sure building the element tree doesn't panic,
        // including the expanded form with log lines and every status.
        let mut data = data_for(today(), mixed());
        data.logs = vec![
            log(1, None, today(), "free-standing"),
            log(2, Some(12), today(), "on a task"),
            log(3, None, date(2026, 11, 13), "another day"),
        ];
        let mut view = DayView::new();
        let _ = view.view();
        view.load(data.clone());
        view.update(Message::ToggleShowDone);
        view.update(Message::ToggleShowNoDate);
        for id in [1, 12, 5, 6, 7] {
            view.update(Message::Expand(id));
            let _ = view.view();
        }
        data.day = date(2026, 11, 20);
        view.load(data);
        let _ = view.view();
    }
}
