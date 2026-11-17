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
//! (`fortnight_core::notes`).

use fortnight_core::domain::{NewTask, Priority, Task, TaskId};
use fortnight_core::notes::{Subtask, join_notes, split_notes};
use fortnight_core::parse;
use fortnight_core::week::add_months;
use iced::widget::overlay::menu;
use iced::widget::{
    Column, Row, Space, button, checkbox, column, combo_box, container, row, text, text_editor,
    text_input,
};
use iced::{Center, Color, Element, Fill, Theme, border};
use jiff::civil::{Date, Weekday};

use super::day::Message;
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
    pub(super) subtasks: Vec<Subtask>,
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
/// for an existing task): project, due, estimate, priority, the error,
/// notes and subtasks.
pub(super) fn form_fields<'a>(
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
}
