//! The task form: the fields of one task, edited in place on the Today tab
//! (`day.rs` wraps it in its inline card) and in the Week tab's task
//! dialog ([`task_dialog`], shown by `main.rs`).
//!
//! The form speaks [`day::Message`](super::day::Message): its edits
//! (`EditTitle`, `PickDue`, `AddSubtask`, …) and its buttons (`Save`,
//! `Close`, `Delete`) are variants of the Today tab's message, so the
//! inline form and the dialog (`main.rs`'s `Message::Dialog`) route them
//! the same way. [`EditForm::handle`] applies the edits; the buttons are
//! left to the owner of the form.
//!
//! Subtasks are kept in the task's notes as Markdown checklist lines
//! (`fortnight_core::notes`). The notes are edited formatted, like the
//! day's notes (`rich_editor`), without the toolbar: `⌘B` / `⌘I` / `⌘U`
//! (or `Ctrl`), `⌘K` for a link, and lists started with `* `, `- ` or
//! `1. ` that carry on with `Enter`.

use std::fmt;

use fortnight_core::domain::{NewTask, Priority, Recurrence, Task, TaskId};
use fortnight_core::notes::{Subtask, join_notes, split_notes};
use fortnight_core::parse;
use fortnight_core::week::add_months;
use iced::widget::overlay::menu;
use iced::widget::{
    Column, Row, Space, button, checkbox, column, combo_box, container, pick_list, row, text,
    text_input,
};
use iced::{Background, Border, Center, Color, Element, Fill, Theme, border};
use jiff::ToSpan;
use jiff::civil::{Date, Weekday};

use super::day::Message;
use super::rich_editor::rich_editor;
use super::rich_notes;

/// The id of a task form's notes editor, for `operation::focus`.
pub const TASK_NOTES_ID: &str = "task-notes";

/// The id of a task form's notes' link prompt.
pub const TASK_NOTES_LINK_ID: &str = "task-notes-link";
use crate::calendar::{Popup, month_picker};
use crate::theme;

// ---------------------------------------------------------------- priority

/// The three priorities the app offers, and the value each stores. Old
/// data's 3 (`!!!`) shows as High.
const PRIORITIES: [(&str, Priority); 3] = [("Low", 0), ("Medium", 1), ("High", 2)];

/// The stored priority as Low (0), Medium (1) or High (2).
fn priority_level(p: Priority) -> Priority {
    p.min(2)
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
    /// The planned day (where the task is shown).
    pub planned: Option<Date>,
    /// The month the planned-day calendar shows, while it is open.
    pub planned_calendar: Option<Date>,
    /// Whether the planned day was explicitly changed or cleared in the form.
    planned_edited: bool,
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
    /// A priority was picked by hand: a suggestion no longer changes it.
    pub priority_picked: bool,
    /// How often the task repeats when marked done.
    pub recurrence: Option<Recurrence>,
    /// Never sent to the LLM: excluded from every digest.
    pub private: bool,
    pub(super) subtasks: Vec<Subtask>,
    pub new_subtask: String,
    /// The notes without the subtasks.
    pub notes: rich_notes::Content,
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
        let notes = rich_notes::Content::with_text(&text);
        let original_notes = notes.text().trim().to_string();
        Self {
            id: task.id,
            title: task.title.clone(),
            project: task.project.clone().unwrap_or_default(),
            projects: combo_box::State::default(),
            planned: task.planned_for,
            planned_calendar: None,
            planned_edited: false,
            due: task.due_on,
            calendar: None,
            week_start: Weekday::Monday,
            estimate: estimate.clone(),
            original_estimate: estimate,
            priority: priority_level(task.priority),
            suggested: None,
            priority_picked: false,
            recurrence: task.recurrence,
            private: task.private,
            subtasks: subtasks.clone(),
            new_subtask: String::new(),
            notes,
            // As the editor writes them (Markdown is tidied), so untouched
            // notes count as unchanged.
            original_split: (subtasks, original_notes),
            error: None,
        }
    }

    /// Builds the updated task from the form, or an error message.
    ///
    /// An unchanged estimate is not re-parsed; a typed subtask not yet added
    /// is added.
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
            planned_for: self.planned,
            estimate_minutes,
            priority,
            private: self.private,
            recurrence: self.recurrence,
            notes,
            ..task.clone()
        })
    }
}

/// The distinct projects of open tasks, sorted case-insensitively.
fn open_projects(tasks: &[Task]) -> Vec<String> {
    let mut out: Vec<String> = tasks
        .iter()
        .filter(|t| t.status.is_open())
        .filter_map(|t| t.project.clone())
        .collect();
    // Case-insensitive, without a lowercase copy per comparison; ties by
    // exact spelling so that exact duplicates end up next to each other.
    out.sort_unstable_by(|a, b| {
        a.chars()
            .flat_map(char::to_lowercase)
            .cmp(b.chars().flat_map(char::to_lowercase))
            .then_with(|| a.cmp(b))
    });
    out.dedup();
    out
}

impl EditForm {
    /// An empty form for a new task (the Week tab's dialog), with the
    /// project list from `tasks`.
    pub fn blank(tasks: &[Task], week_start: Weekday) -> Self {
        open_form(&Task::default(), tasks, week_start)
    }

    /// An empty form for a new task initially planned for `day`, with the
    /// project list from `tasks`.
    pub fn for_new_task(day: Date, tasks: &[Task], week_start: Weekday) -> Self {
        let base = Task {
            planned_for: Some(day),
            ..Task::default()
        };
        open_form(&base, tasks, week_start)
    }

    /// The new task this form describes, defaulting its planned day to `day`
    /// unless the user picked or cleared it, or an error message.
    pub fn new_task(&self, day: Date) -> Result<NewTask, String> {
        let planned_for = if self.planned_edited {
            self.planned
        } else {
            self.planned.or(Some(day))
        };
        let base = Task {
            planned_for,
            ..Task::default()
        };
        let t = self.apply(&base)?;
        Ok(NewTask {
            title: t.title,
            notes: t.notes,
            project: t.project,
            due_on: t.due_on,
            planned_for,
            estimate_minutes: t.estimate_minutes,
            priority: t.priority,
            private: t.private,
            recurrence: t.recurrence,
        })
    }

    /// The decision model suggests `priority`. For a new task (`new`) it
    /// is applied, unless a priority was picked by hand; for an existing
    /// one it is only shown.
    pub fn suggest(&mut self, priority: Option<Priority>, new: bool) {
        self.suggested = priority;
        if new
            && !self.priority_picked
            && let Some(p) = priority
        {
            self.priority = priority_level(p);
        }
    }

    /// What should get the focus after the last edit of the notes (their
    /// link prompt, or the notes again after it), once.
    pub fn take_focus(&mut self) -> Option<&'static str> {
        self.notes.take_focus().map(|f| match f {
            rich_notes::Focus::Editor => TASK_NOTES_ID,
            rich_notes::Focus::LinkPrompt => TASK_NOTES_LINK_ID,
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
            Message::SetPriority(p) => {
                self.priority = priority_level(*p);
                self.priority_picked = true;
            }
            Message::SetRecurrence(recurrence) => self.recurrence = *recurrence,
            Message::TogglePrivate(private) => self.private = *private,
            Message::ToggleCalendar => {
                self.planned_calendar = None;
                self.calendar = match self.calendar {
                    Some(_) => None,
                    None => Some(self.due.unwrap_or(today).first_of_month()),
                };
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
            Message::TogglePlannedCalendar => {
                self.calendar = None;
                self.planned_calendar = match self.planned_calendar {
                    Some(_) => None,
                    None => Some(self.planned.unwrap_or(today).first_of_month()),
                };
            }
            Message::ClosePlannedCalendar => self.planned_calendar = None,
            Message::PlannedCalendarMonth(step) => {
                if let Some(month) = &mut self.planned_calendar {
                    *month = add_months(*month, *step).first_of_month();
                }
            }
            Message::PickPlanned(date) => {
                self.planned = Some(*date);
                self.planned_edited = true;
                self.planned_calendar = None;
            }
            Message::ClearPlanned => {
                self.planned = None;
                self.planned_edited = true;
                self.planned_calendar = None;
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

fn labelled<'a>(label: &'a str, field: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    column![
        text(label).size(theme::TEXT_SMALL).style(theme::muted),
        field.into()
    ]
    .spacing(4)
    .width(Fill)
    .into()
}

pub(super) fn field<'a>(
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

/// A repeat cadence choice with a `Display` impl, for `pick_list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RepeatChoice(Option<Recurrence>);

const REPEAT_CHOICES: [RepeatChoice; 5] = [
    RepeatChoice(None),
    RepeatChoice(Some(Recurrence::Daily)),
    RepeatChoice(Some(Recurrence::Weekdays)),
    RepeatChoice(Some(Recurrence::Weekly)),
    RepeatChoice(Some(Recurrence::Monthly)),
];

impl fmt::Display for RepeatChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.map_or("None", Recurrence::label))
    }
}

/// The repeat picker: None, Daily, Weekdays, Weekly, Monthly.
fn repeat_picker<'a>(current: Option<Recurrence>) -> Element<'a, Message> {
    let list = pick_list(
        REPEAT_CHOICES,
        Some(RepeatChoice(current)),
        |c: RepeatChoice| Message::SetRecurrence(c.0),
    )
    .padding(theme::DROPDOWN_PADDING)
    .handle(pick_list::Handle::None)
    .text_size(theme::TEXT_BODY)
    .width(Fill)
    .style(picker_style)
    .menu_style(project_menu);
    theme::dropdown(list)
}

/// A pick list styled like [`theme::input`].
fn picker_style(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let p = theme.palette();
    let ring = match status {
        pick_list::Status::Opened { .. } => theme::accent_color(),
        pick_list::Status::Hovered => theme::tint(theme, 0.15),
        pick_list::Status::Active => Color::TRANSPARENT,
    };
    pick_list::Style {
        text_color: p.text,
        placeholder_color: theme::muted_color(theme),
        handle_color: theme::muted_color(theme),
        background: Background::Color(theme::tint(theme, 0.075)),
        border: Border {
            radius: theme::RADIUS.into(),
            width: 1.0,
            color: ring,
        },
    }
}

/// A date field: a button showing the date (or muted placeholder) that opens
/// its calendar.
fn date_button<'a>(
    date: Option<Date>,
    today: Date,
    placeholder: &'a str,
    on_press: Message,
) -> Element<'a, Message> {
    let label = match date {
        Some(d) if d.year() == today.year() => {
            text(d.strftime("%a %-d %b").to_string()).size(theme::TEXT_BODY)
        }
        Some(d) => text(d.strftime("%a %-d %b %Y").to_string()).size(theme::TEXT_BODY),
        None => text(placeholder).size(theme::TEXT_BODY).style(theme::muted),
    };
    button(label)
        .width(Fill)
        .height(35)
        .padding([8, 10])
        .style(field_button)
        .on_press(on_press)
        .into()
}

/// The planned-day pop-up: quick picks (`Today`, `Tomorrow`, `+1w`) above the
/// month calendar.
fn planned_popup<'a>(
    month: Date,
    planned: Option<Date>,
    today: Date,
    week_start: Weekday,
) -> Element<'a, Message> {
    let tomorrow = today.saturating_add(1.days());
    let next_week = today.saturating_add(7.days());
    let quick = |label: &'a str, date: Date| {
        button(
            text(label)
                .size(theme::TEXT_SMALL)
                .width(Fill)
                .align_x(Center),
        )
        .width(Fill)
        .padding([5, 8])
        .style(theme::tab_button(planned == Some(date)))
        .on_press(Message::PickPlanned(date))
    };
    // Width matches `calendar::month_picker` (7 * 36 + 2 * SPACING = 276).
    let quick_bar = container(
        row![
            quick("Today", today),
            quick("Tomorrow", tomorrow),
            quick("+1w", next_week),
        ]
        .spacing(4),
    )
    .width(276)
    .padding(6)
    .style(popup_bar_style);
    column![
        quick_bar,
        month_picker(
            month,
            planned,
            None,
            today,
            week_start,
            Message::PlannedCalendarMonth,
            Message::PickPlanned,
            Some(Message::ClearPlanned),
        ),
    ]
    .spacing(6)
    .into()
}

/// The raised panel holding the quick planned-day picks, matching the calendar.
fn popup_bar_style(theme: &Theme) -> container::Style {
    container::Style {
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
    }
}

/// A button that looks like [`theme::input`].
fn field_button(theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(theme::tint(theme, if hovered { 0.11 } else { 0.075 }).into()),
        text_color: theme.palette().text,
        border: border::rounded(theme::RADIUS),
        ..button::Style::default()
    }
}

/// A subtask's card: the input fill, quieter when done.
fn subtask_card(done: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| container::Style {
        background: Some(theme::tint(theme, if done { 0.05 } else { 0.075 }).into()),
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
    // Its list opens below the field whenever it fits there.
    crate::open_below::open_below(
        combo_box(&form.projects, &form.project, current, Message::PickProject)
            .on_input(Message::EditProject)
            .padding([8, 10])
            .size(theme::TEXT_BODY)
            .input_style(theme::input)
            .menu_style(project_menu),
    )
}

/// The project list: a raised panel like the calendar.
fn project_menu(theme: &Theme) -> menu::Style {
    let p = theme.palette();
    menu::Style {
        background: theme::tint(theme, 0.06).into(),
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
/// for an existing task): project, estimate, planned, due, priority, repeat,
/// private, the error, notes and subtasks.
pub(super) fn form_fields<'a>(
    form: &'a EditForm,
    today: Date,
    title_row: Element<'a, Message>,
) -> Column<'a, Message> {
    let editor = rich_editor(&form.notes, Message::EditNotes)
        .id(TASK_NOTES_ID)
        .placeholder("Notes")
        .on_link(Message::OpenLink)
        .min_height(96.0)
        .padding(10)
        .size(theme::TEXT_BODY);
    let mut notes = Column::new().spacing(8).push(editor);
    if let Some(prompt) =
        rich_notes::link_prompt(&form.notes, TASK_NOTES_LINK_ID, Message::EditNotes)
    {
        notes = notes.push(prompt);
    }

    let planned_field = Popup::new(
        date_button(
            form.planned,
            today,
            "No date",
            Message::TogglePlannedCalendar,
        ),
        form.planned_calendar
            .map(|month| planned_popup(month, form.planned, today, form.week_start)),
        Message::ClosePlannedCalendar,
    );
    let due_field = Popup::new(
        date_button(form.due, today, "", Message::ToggleCalendar),
        form.calendar.map(|month| {
            month_picker(
                month,
                form.due,
                None,
                today,
                form.week_start,
                Message::CalendarMonth,
                Message::PickDue,
                Some(Message::ClearDue),
            )
        }),
        Message::CloseCalendar,
    );

    let mut body = column![
        title_row,
        row![
            labelled("Project", project_field(form)),
            labelled(
                "Estimate",
                field("1h30m", &form.estimate, Message::EditEstimate)
            ),
        ]
        .spacing(theme::SPACING),
        row![
            labelled("Planned", planned_field),
            labelled("Due", due_field),
        ]
        .spacing(theme::SPACING),
        row![
            labelled("Priority", priority_picker(form.priority, form.suggested)),
            labelled("Repeat", repeat_picker(form.recurrence)),
        ]
        .spacing(theme::SPACING),
        checkbox(form.private)
            .label("Private (not sent to AI)")
            .text_size(theme::TEXT_SMALL)
            .on_toggle(Message::TogglePrivate),
    ]
    .spacing(theme::SPACING);

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
    let shown_day = if form.planned_edited {
        form.planned
    } else {
        form.planned.or(day)
    };
    let day_label = shown_day.map_or_else(
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

#[cfg(test)]
mod tests {
    use super::*;
    use fortnight_core::domain::TaskStatus;
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
            planned_for: due,
            position: id as f64,
            ..Task::default()
        }
    }

    fn with_status(mut t: Task, status: TaskStatus) -> Task {
        t.status = status;
        t
    }

    #[test]
    fn a_suggestion_sets_a_new_tasks_priority_until_one_is_picked() {
        let mut form = EditForm::blank(&[], Weekday::Monday);
        form.suggest(Some(2), true);
        assert_eq!((form.priority, form.suggested), (2, Some(2)));
        form.handle(&Message::SetPriority(0), date(2026, 11, 18));
        form.suggest(Some(1), true);
        assert_eq!((form.priority, form.suggested), (0, Some(1)));
        // An existing task's is only shown.
        let mut form = EditForm::blank(&[], Weekday::Monday);
        form.suggest(Some(2), false);
        assert_eq!((form.priority, form.suggested), (0, Some(2)));
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
    fn project_list_is_open_tasks_only() {
        let mut a = task(1, "a", None);
        a.project = Some("work".into());
        let mut b = task(2, "b", None);
        b.project = Some("Home".into());
        let mut c = with_status(task(3, "c", None), TaskStatus::Done);
        c.project = Some("old".into());
        let mut d = task(4, "d", None);
        d.project = Some("work".into());
        assert_eq!(
            open_projects(&[a.clone(), b.clone(), c, d.clone()]),
            ["Home", "work"]
        );
        // Exact duplicates go even when another spelling sorts between them.
        let mut e = task(5, "e", None);
        e.project = Some("Work".into());
        let mut f = task(6, "f", None);
        f.project = Some("home".into());
        assert_eq!(
            open_projects(&[a, e, d, b, f]),
            ["Home", "home", "Work", "work"]
        );
    }

    #[test]
    fn planned_day_edit_clear_and_mutual_calendar_exclusion() {
        let mut form = EditForm::for_new_task(today(), &[], Weekday::Monday);
        assert_eq!(form.planned, Some(today()));
        form.handle(&Message::EditTitle("Ship release".into()), today());

        // Opening one calendar closes the other.
        form.handle(&Message::ToggleCalendar, today());
        assert_eq!(form.calendar, Some(date(2026, 11, 1)));
        assert_eq!(form.planned_calendar, None);

        form.handle(&Message::TogglePlannedCalendar, today());
        assert_eq!(form.calendar, None);
        assert_eq!(form.planned_calendar, Some(date(2026, 11, 1)));

        form.handle(&Message::PlannedCalendarMonth(1), today());
        assert_eq!(form.planned_calendar, Some(date(2026, 12, 1)));

        // Quick-pick / picking a planned day updates planned and closes popup.
        let next_week = today().saturating_add(7.days());
        form.handle(&Message::PickPlanned(next_week), today());
        assert_eq!(form.planned, Some(next_week));
        assert_eq!(form.planned_calendar, None);
        let new = form.new_task(today()).unwrap();
        assert_eq!(new.planned_for, Some(next_week));

        // Clearing planned in a new-task form keeps it cleared (does not fall back to default day).
        form.handle(&Message::ClearPlanned, today());
        assert_eq!(form.planned, None);
        let cleared = form.new_task(today()).unwrap();
        assert_eq!(cleared.planned_for, None);

        // Toggle back to due calendar closes planned calendar.
        form.handle(&Message::TogglePlannedCalendar, today());
        assert!(form.planned_calendar.is_some());
        form.handle(&Message::ToggleCalendar, today());
        assert_eq!(form.planned_calendar, None);
        assert!(form.calendar.is_some());
        form.handle(&Message::CloseCalendar, today());
        assert_eq!(form.calendar, None);
    }

    #[test]
    fn recurrence_and_private_round_trip_through_apply_and_new_task() {
        // New task path.
        let mut form = EditForm::for_new_task(today(), &[], Weekday::Monday);
        form.handle(&Message::EditTitle("Pay rent".into()), today());
        form.handle(&Message::SetRecurrence(Some(Recurrence::Monthly)), today());
        form.handle(&Message::TogglePrivate(true), today());
        let new = form.new_task(today()).unwrap();
        assert_eq!(new.recurrence, Some(Recurrence::Monthly));
        assert!(new.private);
        assert_eq!(new.planned_for, Some(today()));

        // Existing task round-trip via from_task -> handle -> apply.
        let mut existing = task(10, "Standup", Some(today()));
        existing.recurrence = Some(Recurrence::Weekdays);
        existing.private = true;
        let mut edit = EditForm::from_task(&existing);
        assert_eq!(edit.recurrence, Some(Recurrence::Weekdays));
        assert!(edit.private);
        assert_eq!(edit.planned, Some(today()));
        assert_eq!(edit.apply(&existing).unwrap(), existing);

        edit.handle(&Message::SetRecurrence(Some(Recurrence::Weekly)), today());
        edit.handle(&Message::TogglePrivate(false), today());
        edit.handle(&Message::PickPlanned(date(2026, 11, 20)), today());
        let updated = edit.apply(&existing).unwrap();
        assert_eq!(updated.recurrence, Some(Recurrence::Weekly));
        assert!(!updated.private);
        assert_eq!(updated.planned_for, Some(date(2026, 11, 20)));

        // Dialog view builds cleanly with both calendars and fields populated.
        edit.handle(&Message::TogglePlannedCalendar, today());
        let _ = task_dialog(&edit, Some(today()), true, today());
    }
}
