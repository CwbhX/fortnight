//! The Today tab: one day's tasks (PLAN.md 4.2 "Today").
//!
//! Sections, top to bottom:
//!
//! - **Recommended** (today and later days): the model's top three open
//!   tasks. On today from the Tasks list (carried-over ones included) and
//!   the overdue tasks; on a later day only from the tasks planned for it.
//!   Most important
//!   first, numbered (the model's reasons aren't shown). They are left out of
//!   the sections below.
//! - **Tasks**: open tasks whose [`Task::day`] (the planned day) is the
//!   shown day, then the add-a-task input. On today this also carries over
//!   open tasks planned for an earlier day whose deadline has not passed.
//! - **Overdue** (only when the shown day is today, and only if any): open
//!   tasks whose deadline has passed.
//! - **Notes** ("Today's Notes" on today): a free-text box for the shown
//!   day, any day. Saved a moment after typing stops (or on moving to
//!   another day), as one free-standing log line dated that day (replacing
//!   its others), so it reaches the weekly summary as `log_lines`. Older
//!   side work lines show up as lines of the notes. Edited formatted
//!   (`rich_editor`): bold, italic, underline, links, bullet / dash /
//!   numbered lists, with a toolbar in the title bar and shortcuts; kept
//!   as Markdown in the text (`rich_notes`).
//! - **Completed**, collapsed: done tasks whose day is the shown day (as on
//!   the Week tab), whenever they were ticked; a done task with no day shows
//!   on the day it was ticked.
//! - **No date**, collapsed: open tasks with no planned day (the inbox),
//!   except, on today, those whose deadline has passed (Overdue).
//! - **Deleted**, collapsed: every deleted task, newest first, each a quiet
//!   card with Restore and Delete forever, and Delete all in the header
//!   when open. Delete moves a task here (status `Dropped`); only Delete
//!   forever (or Delete all) removes it.
//!
//! Each section is a panel in the Week tab's scheme (`panel`): a tinted
//! title bar with the name in bold and the count muted, over Week-style
//! cards. The day's own panel is laid out like its Week column: the weekday
//! (accent on today) and date, a `+` for the new-task dialog, and the same
//! load bar. The collapsed sections are panels whose title bar opens them.
//!
//! The date navigation lives in the top bar.
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
//! clears the selection. Closing a form with the mouse (Save or Close)
//! clears the selection too, so no accent ring is left behind.
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
//! (iced text has none), undo for the notes, and saving notes typed in the
//! last moment before quitting.

use std::cmp::Ordering;
use std::time::{Duration, Instant};

use fortnight_core::domain::{NewTask, Priority, Task, TaskId, TaskStatus};
use fortnight_core::notes::subtask_counts;
use fortnight_core::parse;
use fortnight_core::report::ColumnLoad;
use iced::widget::{
    Column, Row, Space, button, checkbox, column, container, mouse_area, pin, row, scrollable,
    stack, text, text_input, tooltip,
};
use iced::{Center, Color, Element, Fill, Point, Theme, border, mouse};
use jiff::civil::{Date, Weekday};
use jiff::tz::TimeZone;

use super::NavKey;
use super::panel::{self, load_line, load_view};
use super::rich_editor::rich_editor;
use super::rich_notes;
use super::task_form::{EditForm, field, form_fields, open_form};
use crate::{motion, theme};

/// The id of the add-a-task input, for `operation::focus`.
pub const QUICK_ADD_ID: &str = "quick-add";

/// The id of the day's notes editor, for `operation::focus`.
pub const NOTES_ID: &str = "day-notes";

/// The id of the day's notes' link prompt.
pub const NOTES_LINK_ID: &str = "day-notes-link";

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
    /// Editing a task's notes.
    EditNotes(rich_notes::Action),
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
    /// The `+` in the day's title bar: the new-task dialog for the day.
    AddTask,
    /// The add-a-task input under the day's tasks.
    EditQuickAdd(String),
    SubmitQuickAdd,
    /// Typing in Today's Notes. Saved by `main.rs` once typing pauses
    /// ([`DayView::notes_to_save`]).
    EditDayNotes(rich_notes::Action),
    /// A click on a link in some notes.
    OpenLink(String),
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
    /// Create a task (the add-a-task input). `main.rs` records it for undo.
    Add(NewTask),
    /// The Tasks section was rearranged: set each task's position.
    Reorder(Vec<(TaskId, f64)>),
    /// Open the new-task dialog for this day.
    NewTask(Date),
    /// Give a widget the keyboard focus: the day's notes after a toolbar
    /// click, or a notes editor's link prompt.
    Focus(&'static str),
    /// Open a web address from the notes in the browser.
    OpenLink(String),
}

/// A task the model recommends doing on the shown day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recommended {
    pub id: TaskId,
}

/// A free-standing log line, as the view needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// The local date it was written on.
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
    /// The model's recommended tasks for the shown day, most important
    /// first: on today from this week's Recommended tasks report, on a later
    /// day worked out for it. Not shown on past days.
    pub recommended: Vec<Recommended>,
    /// Recommendations are being worked out.
    pub recommending: bool,
    /// Every free-standing log line, oldest first.
    pub logs: Vec<LogLine>,
    /// The shown day's load, as its Week column's bar shows it (`None`:
    /// no bar).
    pub load: Option<ColumnLoad>,
}

#[derive(Debug, Default)]
pub struct DayView {
    data: Option<DayData>,
    /// Where `data`'s tasks are shown, worked out in `load`.
    placement: Placement,
    show_done: bool,
    show_no_date: bool,
    show_deleted: bool,
    /// The keyboard selection highlight.
    selected: Option<TaskId>,
    /// The expanded row's form.
    editing: Option<EditForm>,
    /// The shown day's notes.
    notes: rich_notes::Content,
    /// The day `notes` belong to.
    notes_day: Option<Date>,
    /// When the notes were last typed in, while not yet saved.
    notes_edited: Option<Instant>,
    /// Another day's notes left with unsaved typing: saved straight away.
    notes_left: Option<(Date, String)>,
    /// The add-a-task input.
    quick_add: String,
    /// A held Tasks card.
    drag: Option<Drag>,
    /// The Tasks card under the pointer.
    hovered: Option<TaskId>,
}

/// Where a task is shown on the Today tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Section {
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
    /// Display order of the task sections.
    const ALL: [Section; 6] = [
        Section::Recommended,
        Section::Day,
        Section::Overdue,
        Section::Done,
        Section::NoDate,
        Section::Deleted,
    ];
}

/// The local date a done task was completed on, if known.
fn completed_on(task: &Task, tz: &TimeZone) -> Option<Date> {
    task.completed_at.map(|at| tz.to_datetime(at).date())
}

/// The section `task` belongs in when `day` is shown, or `None` if it is not
/// on screen. See the module doc for the rules.
fn section_of(task: &Task, day: Date, today: Date, tz: &TimeZone) -> Option<Section> {
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
    if day == today {
        // Overdue first, planned day or not (an inbox task too).
        if task.is_overdue(today) {
            return Some(Section::Overdue);
        }
        if task.on_today_list(today) {
            return Some(Section::Day);
        }
    }
    match task.day() {
        None => Some(Section::NoDate),
        Some(d) if d == day => Some(Section::Day),
        Some(_) => None,
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
const MAX_RECOMMENDED: usize = 3;

/// The recommended tasks shown: on today and later days (not past ones),
/// those still on the shown day's list (its Tasks or Overdue section), at
/// most [`MAX_RECOMMENDED`], in the model's order. `main.rs` works them out
/// for the shown day: on today from today's list and the overdue tasks, on
/// a later day from the tasks planned for it.
fn recommended_tasks(data: &DayData) -> Vec<&Task> {
    if data.day < data.today {
        return Vec::new();
    }
    data.recommended
        .iter()
        .filter_map(|r| data.tasks.iter().find(|t| t.id == r.id))
        .filter(|t| {
            matches!(
                section_of(t, data.day, data.today, &data.tz),
                Some(Section::Day | Section::Overdue)
            )
        })
        .take(MAX_RECOMMENDED)
        .collect()
}

/// Where every task is shown for one [`DayData`]: per section (in
/// [`Section::ALL`] order), indices into `DayData::tasks`, sorted.
/// Worked out once in [`DayView::load`], the only place the data changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Placement {
    sections: [Vec<usize>; Section::ALL.len()],
}

impl Placement {
    /// Places every task. Recommended: [`recommended_tasks`], in the
    /// model's order, and left out of the other sections; the rest by
    /// [`section_of`]. Overdue: oldest deadline first, then [`open_order`].
    /// The day: manual order (`position`). No date: [`open_order`].
    /// Completed: most recently finished first. Deleted: most recently
    /// deleted first.
    fn new(data: &DayData) -> Self {
        let recommended: Vec<TaskId> = recommended_tasks(data).iter().map(|t| t.id).collect();
        let mut out = Self::default();
        for (i, task) in data.tasks.iter().enumerate() {
            // Recommended ones are pushed in task order, sorted by rank below.
            let section = if recommended.contains(&task.id) {
                Some(Section::Recommended)
            } else {
                section_of(task, data.day, data.today, &data.tz)
            };
            if let Some(section) = section {
                out.sections[section as usize].push(i);
            }
        }
        let t = |i: &usize| &data.tasks[*i];
        for section in Section::ALL {
            let list = &mut out.sections[section as usize];
            match section {
                Section::Recommended => list.sort_by_key(|i| {
                    recommended
                        .iter()
                        .position(|&id| id == data.tasks[*i].id)
                        .unwrap_or(usize::MAX)
                }),
                Section::Overdue => list.sort_by(|a, b| {
                    let (a, b) = (t(a), t(b));
                    a.due_on.cmp(&b.due_on).then(open_order(a, b))
                }),
                // The day's tasks are in manual order (drag to rearrange).
                Section::Day => list.sort_by(|a, b| {
                    let (a, b) = (t(a), t(b));
                    a.position.total_cmp(&b.position).then(a.id.cmp(&b.id))
                }),
                Section::NoDate => list.sort_by(|a, b| open_order(t(a), t(b))),
                Section::Done | Section::Deleted => list.sort_by(|a, b| {
                    let (a, b) = (t(a), t(b));
                    let when = |t: &Task| t.completed_at.unwrap_or(t.updated_at);
                    when(b).cmp(&when(a)).then(a.id.cmp(&b.id))
                }),
            }
        }
        out
    }

    /// The tasks in `section`, in display order.
    fn tasks<'a>(&self, data: &'a DayData, section: Section) -> Vec<&'a Task> {
        self.sections[section as usize]
            .iter()
            .map(|&i| &data.tasks[i])
            .collect()
    }

    /// The section task `id` is shown in, if any.
    fn section_of_id(&self, data: &DayData, id: TaskId) -> Option<Section> {
        Section::ALL.into_iter().find(|&s| {
            self.sections[s as usize]
                .iter()
                .any(|&i| data.tasks[i].id == id)
        })
    }

    /// The rows the keyboard moves through, top to bottom, skipping
    /// collapsed sections and Deleted.
    fn visible_rows(&self, data: &DayData, show_done: bool, show_no_date: bool) -> Vec<TaskId> {
        Section::ALL
            .into_iter()
            .filter(|s| match s {
                Section::Done => show_done,
                Section::NoDate => show_no_date,
                Section::Deleted => false,
                _ => true,
            })
            .flat_map(|s| self.sections[s as usize].iter().map(|&i| data.tasks[i].id))
            .collect()
    }
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
fn due_label(due: Date, today: Date) -> String {
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
        if self.notes_day != Some(data.day) {
            // Another day: what was typed on the last one is saved first.
            if self.notes_edited.take().is_some()
                && let Some(day) = self.notes_day
            {
                self.notes_left = Some((day, self.notes.text().trim_end().to_string()));
            }
            self.notes = rich_notes::Content::with_text(&day_notes(&data, data.day));
            self.notes_day = Some(data.day);
        } else if self.notes_edited.is_none() {
            // Unsaved typing wins over what is stored. Compared trimmed at
            // both ends, as `main.rs` saves them, so an autosave of notes
            // with leading blank space doesn't rewrite the editor.
            // Either as stored (an empty `1. ` item at the end is saved
            // as `1.`) or as the editor writes them (old Markdown tidied).
            let stored = day_notes(&data, data.day);
            let shown = self.notes.text();
            let tidied = rich_notes::Content::with_text(&stored);
            if shown.trim() != stored.trim() && shown.trim() != tidied.text().trim() {
                self.notes = tidied;
            }
        }
        self.placement = Placement::new(&data);
        self.data = Some(data);

        // A card that left the Tasks section sends no exit event.
        if let Some(id) = self.hovered
            && self.section_of_id(id) != Some(Section::Day)
        {
            self.hovered = None;
        }

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
                    // The form replaces the card, which sends no exit.
                    self.hovered = None;
                }
                Action::None
            }
            NavKey::Close => {
                if let Some(form) = self.editing.as_mut()
                    && form.calendar.is_some()
                {
                    // Only the form's due calendar closes.
                    form.calendar = None;
                } else if self.editing.is_some() {
                    self.editing = None;
                    self.hovered = None;
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
            return form.take_focus().map_or(Action::None, Action::Focus);
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
                // The cards move under the pointer without enter/exit events.
                self.hovered = None;
                let order: Vec<&Task> = self.placement.tasks(data, Section::Day);
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
            Message::AddTask => Action::NewTask(data.day),
            Message::Expand(id) => {
                // The card is replaced by the form (or back) with no exit event.
                self.hovered = None;
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
                match parse::quick_add_task(&self.quick_add, data.today, Some(data.day)) {
                    Some(new) => {
                        self.quick_add.clear();
                        Action::Add(new)
                    }
                    None => Action::None,
                }
            }
            Message::EditDayNotes(action) => {
                let format = matches!(action, rich_notes::Action::Format(_));
                if action.is_edit() {
                    self.notes_edited = Some(Instant::now());
                }
                self.notes.perform(action);
                match self.notes.take_focus() {
                    Some(rich_notes::Focus::LinkPrompt) => Action::Focus(NOTES_LINK_ID),
                    // A toolbar click took the focus away.
                    Some(rich_notes::Focus::Editor) => Action::Focus(NOTES_ID),
                    None if format => Action::Focus(NOTES_ID),
                    None => Action::None,
                }
            }
            Message::OpenLink(url) => Action::OpenLink(url),
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
                        self.hovered = None;
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
                Some(form) => {
                    self.selected = None;
                    self.hovered = None;
                    Action::SetStatus(form.id, TaskStatus::Dropped)
                }
                None => Action::None,
            },
            Message::Close => {
                self.editing = None;
                self.selected = None;
                self.hovered = None;
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
        self.placement.section_of_id(self.data.as_ref()?, id)
    }

    fn visible(&self) -> Vec<TaskId> {
        self.data
            .as_ref()
            .map(|d| {
                self.placement
                    .visible_rows(d, self.show_done, self.show_no_date)
            })
            .unwrap_or_default()
    }

    // ------------------------------------------------------------- view

    pub fn view(&self) -> Element<'_, Message> {
        let Some(data) = &self.data else {
            return text("Loading…").style(theme::muted).into();
        };
        let mut content = Column::new().spacing(theme::SPACING * 1.5);

        let recommended = self.placement.tasks(data, Section::Recommended);
        if !recommended.is_empty() {
            let head = panel_head("Recommended", Some(recommended.len()), text::default);
            content = content.push(panel::panel(head, Some(self.rows(recommended, data, true))));
        } else if data.recommending && data.day >= data.today {
            let head = panel_head("Recommended", None, text::default);
            let body = text("Picking the top tasks…")
                .size(theme::TEXT_SMALL)
                .style(theme::muted);
            content = content.push(panel::panel(head, Some(body.into())));
        }

        content = content.push(self.day_panel(data));

        let overdue = self.placement.tasks(data, Section::Overdue);
        if !overdue.is_empty() {
            let head = panel_head("Overdue", Some(overdue.len()), theme::danger);
            content = content.push(panel::panel(head, Some(self.rows(overdue, data, false))));
        }

        content = content.push(self.day_notes_view(data));

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
            let tasks = self.placement.tasks(data, section);
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
                .padding(CARD_PADDING)
                .width(theme::CONTENT_WIDTH - theme::SPACING * 6.0)
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

    /// Whether some day's notes have unsaved typing (`main.rs` then checks
    /// [`Self::notes_to_save`] every so often).
    pub fn notes_pending(&self) -> bool {
        self.notes_edited.is_some() || self.notes_left.is_some()
    }

    /// A day's notes to save: those of a day left with unsaved typing
    /// straight away, the shown day's once typing has paused for
    /// [`NOTES_SAVE_DELAY`] (then they count as saved).
    pub fn notes_to_save(&mut self, now: Instant) -> Option<(Date, String)> {
        if let Some(left) = self.notes_left.take() {
            return Some(left);
        }
        let edited = self.notes_edited?;
        if now.saturating_duration_since(edited) < NOTES_SAVE_DELAY {
            return None;
        }
        self.notes_edited = None;
        Some((self.notes_day?, self.notes.text().trim_end().to_string()))
    }

    /// The shown day's notes, saved as you type: "Today's Notes" on today,
    /// "Notes" on any other day. While not being edited (and not empty)
    /// they show formatted; a click edits them.
    fn day_notes_view<'a>(&'a self, data: &'a DayData) -> Element<'a, Message> {
        let heading = if data.day == data.today {
            "Today's Notes"
        } else {
            "Notes"
        };
        let editor = rich_editor(&self.notes, Message::EditDayNotes)
            .id(NOTES_ID)
            .placeholder("What's on your mind?")
            .on_link(Message::OpenLink)
            .min_height(120.0)
            .padding(CARD_PADDING)
            .size(theme::TEXT_BODY);
        let mut body = Column::new().spacing(8).push(editor);
        if let Some(prompt) =
            rich_notes::link_prompt(&self.notes, NOTES_LINK_ID, Message::EditDayNotes)
        {
            body = body.push(prompt);
        }
        let head = row![
            panel_head(heading, None, text::default),
            Space::new().width(Fill),
            rich_notes::toolbar(|cmd| Message::EditDayNotes(rich_notes::Action::Format(cmd))),
        ]
        .align_y(Center);
        panel::panel(head.into(), Some(body.into()))
    }

    /// The day's panel, in the scheme of a Week column: a title bar with
    /// the weekday (accent on today), the date, the `+` and the load bar;
    /// under it the day's cards (or "Nothing planned") and the add-a-task
    /// input.
    fn day_panel<'a>(&'a self, data: &'a DayData) -> Element<'a, Message> {
        let name_style: fn(&Theme) -> text::Style = if data.day == data.today {
            theme::accent
        } else {
            text::default
        };
        let add = tooltip(
            button(theme::icon(theme::Icon::Plus))
                .padding(2)
                .style(theme::icon_button)
                .on_press(Message::AddTask),
            theme::tip("Add a task"),
            tooltip::Position::Bottom,
        );
        let mut head = Column::new().spacing(6).push(
            row![
                text(data.day.strftime("%A").to_string())
                    .size(theme::TEXT_BODY)
                    .font(theme::FONT_BOLD)
                    .style(name_style),
                text(data.day.strftime("%-d %b").to_string())
                    .size(theme::TEXT_SMALL)
                    .style(theme::muted),
                Space::new().width(Fill),
                add,
            ]
            .spacing(6)
            .align_y(Center),
        );
        if let Some(load) = &data.load {
            head = head.push(load_view(&load_line(load)));
        }

        let day_tasks = self.placement.tasks(data, Section::Day);
        let quick_add = text_input("Add a task", &self.quick_add)
            .id(QUICK_ADD_ID)
            .on_input(Message::EditQuickAdd)
            .on_submit(Message::SubmitQuickAdd)
            .padding(CARD_PADDING)
            .size(theme::TEXT_BODY)
            .style(theme::input);
        let list: Element<'a, Message> = if day_tasks.is_empty() {
            container(
                text("Nothing planned")
                    .size(theme::TEXT_SMALL)
                    .style(theme::muted),
            )
            .padding(iced::Padding::ZERO.bottom(theme::SPACING / 2.0))
            .into()
        } else {
            self.sortable_rows(day_tasks, data)
        };
        panel::panel(head.into(), Some(column![list, quick_add].into()))
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
            let id = task.id;
            if let Some(form) = self.editing.as_ref().filter(|f| f.id == id) {
                let form = edit_card(task, form, data.today);
                list = list.push(motion::unfold(true, form).key(id as u64));
                continue;
            }
            let held = dragging.is_some_and(|d| d.id == id);
            let line = !held && over == Some(Spot::Before(id));
            let card = container(card_content(task, data.today, None))
                .padding(CARD_PADDING)
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
            list = list.push(motion::unfold(false, area).key(id as u64));
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

    /// A collapsed section: a panel whose title bar opens it (arrow, name,
    /// count), with Delete all at the right of Deleted's when open.
    fn collapsible<'a>(
        &'a self,
        section: Section,
        label: &'a str,
        open: bool,
        toggle: Message,
        tasks: Vec<&'a Task>,
        data: &'a DayData,
    ) -> Element<'a, Message> {
        let toggle = button(
            row![
                theme::disclosure(open),
                text(label).size(theme::TEXT_BODY).font(theme::FONT_BOLD),
                text(tasks.len().to_string()).size(theme::TEXT_SMALL),
            ]
            .spacing(8)
            .align_y(Center),
        )
        .padding(0)
        .width(Fill)
        .style(theme::text_button)
        .on_press(toggle);
        let mut head = row![toggle].align_y(Center);
        if open && section == Section::Deleted {
            let trash = theme::icon(theme::Icon::Trash).style(|theme: &Theme, _| {
                iced::widget::svg::Style {
                    color: Some(theme::danger_color(theme)),
                }
            });
            head = head.push(tooltip(
                button(trash)
                    .padding(2)
                    .style(theme::danger_button)
                    .on_press(Message::DeleteAll),
                theme::tip("Delete all"),
                tooltip::Position::Bottom,
            ));
        }
        // Built closed too, to be drawn folding away.
        let body = if section == Section::Deleted {
            Column::with_children(tasks.into_iter().map(deleted_row))
                .spacing(6)
                .into()
        } else {
            self.rows(tasks, data, false)
        };
        panel::folding_panel(head.into(), open, body, section as u64)
    }

    /// The rows of a section. `ranked` (the Recommended section only)
    /// numbers them 1, 2, 3 in the order shown.
    fn rows<'a>(
        &'a self,
        tasks: Vec<&'a Task>,
        data: &'a DayData,
        ranked: bool,
    ) -> Element<'a, Message> {
        // A task opening into its form grows from its card's height.
        Column::with_children(tasks.into_iter().enumerate().map(|(i, task)| {
            let (open, row) = match &self.editing {
                Some(form) if form.id == task.id => (true, edit_card(task, form, data.today)),
                _ => {
                    let rank = ranked.then_some(i + 1);
                    let selected = self.selected == Some(task.id);
                    (false, task_row(task, data.today, selected, rank))
                }
            };
            motion::unfold(open, row).key(task.id as u64).into()
        }))
        .spacing(6)
        .into()
    }
}

/// A panel's heading, as a Week column's: the name in bold (in `style`),
/// then the count, muted.
fn panel_head<'a>(
    name: &'a str,
    count: Option<usize>,
    style: fn(&Theme) -> text::Style,
) -> Element<'a, Message> {
    let mut head = row![
        text(name)
            .size(theme::TEXT_BODY)
            .font(theme::FONT_BOLD)
            .style(style)
    ]
    .spacing(8)
    .align_y(Center);
    if let Some(n) = count {
        head = head.push(
            text(n.to_string())
                .size(theme::TEXT_SMALL)
                .style(theme::muted),
        );
    }
    head.into()
}

/// A day's notes: the free-standing log lines dated `day`, oldest first,
/// one per line. (Side work logged before the notes existed shows up as
/// lines of them.)
fn day_notes(data: &DayData, day: Date) -> String {
    data.logs
        .iter()
        .filter(|l| l.date == day)
        .map(|l| l.text.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// How long typing must pause before Today's Notes are saved.
const NOTES_SAVE_DELAY: Duration = Duration::from_millis(1200);

/// Padding inside a task card (and the inputs that sit with them).
const CARD_PADDING: [u16; 2] = [10, 12];

/// A task card, as in the Week tab: a raised fill, lighter on hover,
/// quieter when done, with an accent ring when selected.
fn task_card_style(done: bool, selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(panel::card_fill(theme, done, hovered || selected).into()),
            text_color: theme.palette().text,
            border: border::rounded(panel::CARD_RADIUS)
                .width(1)
                .color(if selected {
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
    let (priority, style): (Priority, fn(&Theme) -> text::Style) = if open {
        (task.priority, text::default)
    } else {
        (0, theme::muted)
    };
    line = line.push(theme::title_with_marks(
        task.title.as_str(),
        priority,
        style,
        |t| t.size(theme::TEXT_BODY),
    ));
    // The project, muted, as on a Week card.
    if let Some(project) = &task.project {
        line = line.push(
            text(format!("#{project}"))
                .size(theme::TEXT_SMALL)
                .style(theme::muted),
        );
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
        .padding(CARD_PADDING)
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
/// whose position changes. `None` if `dragged` isn't in the list, it is
/// let go on itself or before a card not in the list, or nothing moves.
fn reorder(order: &[&Task], dragged: TaskId, spot: Spot) -> Option<Vec<(TaskId, f64)>> {
    let moving = order.iter().find(|t| t.id == dragged)?;
    let mut ids: Vec<&Task> = order.iter().copied().filter(|t| t.id != dragged).collect();
    let at = match spot {
        // Let go on itself: nothing moves.
        Spot::Before(id) if id == dragged => return None,
        // A card that is no longer in the list: no place to land.
        Spot::Before(id) => ids.iter().position(|t| t.id == id)?,
        Spot::End => ids.len(),
    };
    ids.insert(at, moving);
    let changes: Vec<(TaskId, f64)> = ids
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id, (i + 1) as f64))
        .filter(|(id, pos)| order.iter().any(|t| t.id == *id && t.position != *pos))
        .collect();
    (!changes.is_empty()).then_some(changes)
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
            .style(|theme: &Theme| container::Style {
                background: Some(panel::card_fill(theme, false, false).into()),
                border: border::rounded(panel::CARD_RADIUS),
                ..container::Style::default()
            }),
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
            load: None,
        }
    }

    fn loaded(tasks: Vec<Task>) -> DayView {
        let mut view = DayView::new();
        view.load(data_for(today(), tasks));
        view
    }

    fn section_tasks(data: &DayData, section: Section) -> Vec<&Task> {
        Placement::new(data).tasks(data, section)
    }

    fn visible_rows(data: &DayData, show_done: bool, show_no_date: bool) -> Vec<TaskId> {
        Placement::new(data).visible_rows(data, show_done, show_no_date)
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
        // Let go on itself, or before a card that is gone: nothing moves
        // (not to the end).
        assert_eq!(reorder(&order, 1, Spot::Before(1)), None);
        assert_eq!(reorder(&order, 1, Spot::Before(9)), None);
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

    /// A task's notes are edited formatted: `* ` starts a list, `Enter`
    /// carries it on, `⌘B` bolds what is typed next, `⌘K` asks for an
    /// address and focuses its prompt.
    #[test]
    fn task_notes_take_markdown_shortcuts() {
        use rich_notes::Action as A;
        let mut t = task(4, "Notes", None);
        t.planned_for = Some(today());
        let mut view = loaded(vec![t]);
        view.update(Message::Expand(4));
        let mut edit = |a| view.update(Message::EditNotes(a));
        edit(A::Insert("* call ".into()));
        edit(A::Format(rich_notes::Command::Bold));
        edit(A::Insert("Sam".into()));
        edit(A::Enter);
        edit(A::Insert("docs".into()));
        assert_eq!(
            edit(A::Format(rich_notes::Command::Link)),
            Action::Focus(super::super::task_form::TASK_NOTES_LINK_ID)
        );
        edit(A::LinkInput("https://x.y".into()));
        assert_eq!(
            edit(A::LinkSubmit),
            Action::Focus(super::super::task_form::TASK_NOTES_ID)
        );
        assert_eq!(
            view.editing.as_ref().unwrap().notes.text(),
            "• call **Sam**\n• [docs](https://x.y)"
        );
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
        view.update(Message::EditNotes(rich_notes::Action::Paste(
            "some notes".into(),
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
            .map(|id| Recommended { id })
            .collect();
        // Done and missing ids are skipped; at most three.
        fn ids(data: &DayData, s: Section) -> Vec<TaskId> {
            section_tasks(data, s).iter().map(|t| t.id).collect()
        }
        assert_eq!(ids(&data, Section::Recommended), [4, 2, 1]);
        assert_eq!(ids(&data, Section::Day), [3, 5]);
        assert_eq!(visible_rows(&data, false, false), [4, 2, 1, 3, 5]);

        // A past day: no recommendations. A later one shows its own (as
        // `main.rs` gives them), only those planned for it.
        data.day = date(2026, 11, 15);
        assert!(ids(&data, Section::Recommended).is_empty());
        let tue = date(2026, 11, 17);
        data.day = tue;
        assert!(ids(&data, Section::Recommended).is_empty());
        for id in [2, 4] {
            let t = data.tasks.iter_mut().find(|t| t.id == id).unwrap();
            t.planned_for = Some(tue);
        }
        assert_eq!(ids(&data, Section::Recommended), [4, 2]);
    }

    /// A stale list: a recommended task ticked off or moved to another day
    /// leaves Recommended, and the rest close up (they are numbered by
    /// their place in this list).
    #[test]
    fn recommended_keeps_only_tasks_still_on_the_shown_day() {
        let tue = date(2026, 11, 17);
        let mut tasks: Vec<Task> = (1..=4)
            .map(|i| planned(task(i, &format!("t{i}"), None), tue))
            .collect();
        tasks[0] = done_on(tasks[0].clone(), tue);
        tasks[2].planned_for = Some(date(2026, 11, 19));
        let mut data = data_for(tue, tasks);
        data.recommended = [1, 2, 3, 4]
            .into_iter()
            .map(|id| Recommended { id })
            .collect();
        assert_eq!(ids(&section_tasks(&data, Section::Recommended)), [2, 4]);
        assert_eq!(ids(&section_tasks(&data, Section::Done)), [1]);
        // Today: an overdue inbox task can be recommended.
        let mut inbox = task(5, "inbox", None);
        inbox.due_on = Some(date(2026, 11, 13));
        let mut data = data_for(today(), vec![inbox, task(6, "other day", Some(tue))]);
        data.recommended = vec![Recommended { id: 6 }, Recommended { id: 5 }];
        assert_eq!(ids(&section_tasks(&data, Section::Recommended)), [5]);
    }

    #[test]
    fn an_inbox_task_past_its_deadline_is_overdue_on_today() {
        let mut inbox = task(1, "inbox", None);
        inbox.due_on = Some(date(2026, 11, 13));
        assert_eq!(
            section_of(&inbox, today(), today(), &TimeZone::UTC),
            Some(Section::Overdue)
        );
        // On another day it stays in No date.
        assert_eq!(
            section_of(&inbox, date(2026, 11, 18), today(), &TimeZone::UTC),
            Some(Section::NoDate)
        );
        let data = data_for(today(), vec![inbox]);
        assert_eq!(ids(&section_tasks(&data, Section::Overdue)), [1]);
        assert!(section_tasks(&data, Section::NoDate).is_empty());
    }

    #[test]
    fn hover_is_cleared_where_the_card_goes_without_an_exit() {
        let a = planned(task(1, "a", None), today());
        let b = planned(task(2, "b", None), today());
        let mut view = loaded(vec![a.clone(), b.clone()]);
        // Opening the form, then closing it.
        view.update(Message::Enter(Spot::Before(1)));
        view.update(Message::Expand(1));
        assert_eq!(view.hovered, None);
        view.update(Message::Enter(Spot::Before(1)));
        view.update(Message::Close);
        assert_eq!(view.hovered, None);
        // A drop.
        view.update(Message::Press(2));
        view.update(Message::Moved(Point::new(0.0, 0.0)));
        view.update(Message::Moved(Point::new(0.0, 40.0)));
        view.update(Message::Enter(Spot::Before(1)));
        view.update(Message::DropOn(Spot::Before(1)));
        assert_eq!(view.hovered, None);
        // Save and Delete from the form.
        view.update(Message::Expand(1));
        view.update(Message::Enter(Spot::Before(2)));
        view.update(Message::Save);
        assert_eq!(view.hovered, None);
        view.update(Message::Expand(1));
        view.update(Message::Enter(Spot::Before(2)));
        view.update(Message::Delete);
        assert_eq!(view.hovered, None);
        // A reload where the hovered card has left the Tasks section.
        view.update(Message::Enter(Spot::Before(2)));
        view.load(data_for(
            today(),
            vec![a.clone(), done_on(b.clone(), today())],
        ));
        assert_eq!(view.hovered, None);
        // ...but not one where it stays.
        view.update(Message::Enter(Spot::Before(1)));
        view.load(data_for(today(), vec![a, b]));
        assert_eq!(view.hovered, Some(1));
    }

    #[test]
    fn deleting_from_the_form_clears_the_selection() {
        let mut view = loaded(vec![
            task(1, "a", Some(today())),
            task(2, "b", Some(today())),
        ]);
        view.update(Message::Expand(1));
        assert_eq!(view.selected, Some(1));
        view.update(Message::Delete);
        assert_eq!(view.selected, None);
        let tasks = vec![
            with_status(task(1, "a", Some(today())), TaskStatus::Dropped),
            task(2, "b", Some(today())),
        ];
        view.load(data_for(today(), tasks));
        assert_eq!(view.selected, None);
    }

    #[test]
    fn escape_with_the_due_calendar_open_closes_only_the_calendar() {
        let mut view = loaded(vec![task(1, "a", Some(today()))]);
        view.update(Message::Expand(1));
        view.update(Message::EditTitle("typed".into()));
        view.update(Message::ToggleCalendar);
        view.on_key(NavKey::Close);
        let form = view.editing.as_ref().expect("the form stays open");
        assert_eq!(form.calendar, None);
        assert_eq!(form.title, "typed");
        view.on_key(NavKey::Close);
        assert!(view.editing.is_none());
    }

    /// `main.rs` saves notes trimmed at both ends; the reload after that
    /// must not rewrite what is in the editor.
    #[test]
    fn an_autosave_does_not_rewrite_notes_with_leading_space() {
        let mut view = loaded(vec![]);
        view.update(Message::EditDayNotes(rich_notes::Action::Paste(
            "  - item".into(),
        )));
        let (day, saved) = view
            .notes_to_save(Instant::now() + NOTES_SAVE_DELAY)
            .unwrap();
        let mut data = data_for(today(), vec![]);
        data.logs = vec![LogLine {
            date: day,
            text: saved.trim().to_string(),
        }];
        view.load(data);
        assert_eq!(view.notes.text(), "  - item");
    }

    #[test]
    fn the_plus_opens_the_new_task_dialog_for_the_shown_day() {
        let mut view = loaded(Vec::new());
        assert_eq!(view.update(Message::AddTask), Action::NewTask(today()));
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

    /// A toolbar button formats the notes and gives them the focus back
    /// (the click took it); `⌘K` focuses the link prompt.
    #[test]
    fn toolbar_formats_and_refocuses_the_notes() {
        use rich_notes::Action as A;
        let mut view = loaded(vec![]);
        view.update(Message::EditDayNotes(A::Insert("hi".into())));
        view.update(Message::EditDayNotes(A::SelectAll));
        assert_eq!(
            view.update(Message::EditDayNotes(A::Format(rich_notes::Command::Bold))),
            Action::Focus(NOTES_ID)
        );
        assert_eq!(view.notes.text(), "**hi**");
        assert!(view.notes_pending());
        assert_eq!(
            view.update(Message::EditDayNotes(A::Format(rich_notes::Command::Link))),
            Action::Focus(NOTES_LINK_ID)
        );
        assert_eq!(
            view.update(Message::EditDayNotes(A::LinkCancel)),
            Action::Focus(NOTES_ID)
        );
        assert_eq!(
            view.update(Message::OpenLink("https://x.y".into())),
            Action::OpenLink("https://x.y".into())
        );
    }

    /// An empty list item saved (trimmed to `1.`) and reloaded stays a
    /// list item in the editor.
    #[test]
    fn an_autosave_keeps_an_empty_list_item() {
        let mut view = loaded(vec![]);
        view.update(Message::EditDayNotes(rich_notes::Action::Format(
            rich_notes::Command::Numbers,
        )));
        let (day, saved) = view
            .notes_to_save(Instant::now() + NOTES_SAVE_DELAY)
            .unwrap();
        let mut data = data_for(today(), vec![]);
        data.logs = vec![LogLine {
            date: day,
            text: saved.trim().to_string(),
        }];
        view.load(data);
        assert_eq!(view.notes.text(), "1. ");
    }

    #[test]
    fn todays_notes_load_and_save_after_a_pause() {
        let mut data = data_for(today(), vec![]);
        let line = |date, text: &str| LogLine {
            date,
            text: text.into(),
        };
        // Old side work lines become lines of the notes; other days' don't.
        data.logs = vec![
            line(today(), "helped Sam"),
            line(date(2026, 11, 15), "Sunday"),
            line(today(), "standup ran long"),
        ];
        let mut view = DayView::new();
        view.load(data.clone());
        assert_eq!(view.notes.text(), "helped Sam\nstandup ran long");
        assert!(!view.notes_pending());

        view.update(Message::EditDayNotes(rich_notes::Action::Move {
            motion: rich_notes::Motion::DocEnd,
            select: false,
        }));
        assert!(!view.notes_pending(), "moving the cursor is no edit");
        view.update(Message::EditDayNotes(rich_notes::Action::Insert(
            "!".into(),
        )));
        assert!(view.notes_pending());
        let typed = view.notes_edited.unwrap();
        assert_eq!(view.notes_to_save(typed), None, "still typing");
        // A reload while typing keeps the typing.
        view.load(data_for(today(), vec![]));
        assert_eq!(view.notes.text(), "helped Sam\nstandup ran long!");
        assert_eq!(
            view.notes_to_save(typed + NOTES_SAVE_DELAY),
            Some((today(), "helped Sam\nstandup ran long!".to_string()))
        );
        assert!(!view.notes_pending());
        assert_eq!(view.notes_to_save(typed + NOTES_SAVE_DELAY * 2), None);

        // Another day shows its own notes; typing there and moving on saves
        // them for that day straight away.
        let sunday = date(2026, 11, 15);
        let mut other = data.clone();
        other.day = sunday;
        view.load(other);
        assert_eq!(view.notes.text(), "Sunday");
        view.update(Message::EditDayNotes(rich_notes::Action::Move {
            motion: rich_notes::Motion::DocEnd,
            select: false,
        }));
        view.update(Message::EditDayNotes(rich_notes::Action::Insert(
            "?".into(),
        )));
        view.load(data);
        assert_eq!(view.notes.text(), "helped Sam\nstandup ran long");
        assert_eq!(
            view.notes_to_save(Instant::now()),
            Some((sunday, "Sunday?".to_string()))
        );
        assert!(!view.notes_pending());
    }

    #[test]
    fn view_builds_in_every_state() {
        let mut data = data_for(today(), mixed());
        data.logs = vec![LogLine {
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
