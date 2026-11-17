//! The Tasks view: quick-add bar and sections. Owner: agent E.
//!
//! Layout: a quick-add bar with a syntax hint, then the open tasks grouped
//! into Overdue, Today, Next 7 days, Later and No date (empty groups are
//! hidden), then a "Done" toggle that reveals completed tasks. Clicking a
//! row expands it in place into an edit form; one row at a time.
//!
//! The logic lives in small pure functions ([`section_of`], [`due_label`],
//! [`sorted_section`], [`EditForm::apply`]) so it can be tested without a
//! window.
//!
//! Skipped for the prototype: keyboard navigation, undo, drag ordering,
//! delete confirmation, strikethrough on done tasks (iced text has none).

use fortnight_core::domain::{NewTask, Task, TaskId};
use fortnight_core::parse;
use iced::widget::{
    Column, Space, button, checkbox, column, container, operation, row, scrollable, text,
    text_editor, text_input,
};
use iced::{Center, Element, Fill};
use jiff::civil::Date;

use crate::theme;

const QUICK_ADD_ID: &str = "tasks-quick-add";

#[derive(Debug, Clone)]
pub enum Message {
    QuickAddChanged(String),
    QuickAddSubmitted,
    /// The checkbox on a row.
    Toggle(TaskId, bool),
    /// A click on a collapsed row: expand it (or collapse it if open).
    Expand(TaskId),
    EditTitle(String),
    EditProject(String),
    EditDue(String),
    EditNotes(text_editor::Action),
    Save,
    Delete,
    Close,
    /// Show or hide the Done section.
    ToggleShowDone,
}

/// What `main.rs` must do. Contract: do not change without the orchestrator.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Add(NewTask),
    SetDone(TaskId, bool),
    /// Save the editable fields (title, notes, project, due_on).
    Update(Task),
    Delete(TaskId),
}

#[derive(Debug, Default)]
pub struct TasksView {
    quick_add: String,
    show_done: bool,
    editing: Option<EditForm>,
}

/// Where a task is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Overdue,
    Today,
    Next7Days,
    Later,
    NoDate,
    Done,
}

impl Section {
    /// The open-task sections, in display order (Done is drawn separately).
    pub const OPEN: [Section; 5] = [
        Section::Overdue,
        Section::Today,
        Section::Next7Days,
        Section::Later,
        Section::NoDate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Overdue => "Overdue",
            Section::Today => "Today",
            Section::Next7Days => "Next 7 days",
            Section::Later => "Later",
            Section::NoDate => "No date",
            Section::Done => "Done",
        }
    }
}

/// Days from `today` to `date` (negative in the past).
fn days_between(today: Date, date: Date) -> i64 {
    today
        .until(date)
        .map(|span| i64::from(span.get_days()))
        .unwrap_or(0)
}

/// The section a task belongs in. Anything not open (done, dropped) is Done.
/// "Next 7 days" is tomorrow through today + 7.
pub fn section_of(task: &Task, today: Date) -> Section {
    if !task.status.is_open() {
        return Section::Done;
    }
    match task.due_on {
        None => Section::NoDate,
        Some(due) => match days_between(today, due) {
            d if d < 0 => Section::Overdue,
            0 => Section::Today,
            1..=7 => Section::Next7Days,
            _ => Section::Later,
        },
    }
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

/// True when the due date is before today and the task is still open.
pub fn is_overdue(task: &Task, today: Date) -> bool {
    section_of(task, today) == Section::Overdue
}

/// The tasks in `section`, sorted. Open sections: by due date, then
/// position, then id. Done: most recently completed first.
pub fn sorted_section(tasks: &[Task], section: Section, today: Date) -> Vec<&Task> {
    let mut out: Vec<&Task> = tasks
        .iter()
        .filter(|t| section_of(t, today) == section)
        .collect();
    if section == Section::Done {
        out.sort_by(|a, b| b.completed_at.cmp(&a.completed_at).then(a.id.cmp(&b.id)));
    } else {
        out.sort_by(|a, b| {
            a.due_on
                .cmp(&b.due_on)
                .then(a.position.total_cmp(&b.position))
                .then(a.id.cmp(&b.id))
        });
    }
    out
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
    pub notes: text_editor::Content,
    pub error: Option<String>,
}

impl EditForm {
    pub fn from_task(task: &Task) -> Self {
        let due = task.due_on.map(|d| d.to_string()).unwrap_or_default();
        Self {
            id: task.id,
            title: task.title.clone(),
            project: task.project.clone().unwrap_or_default(),
            original_due: due.clone(),
            due,
            notes: text_editor::Content::with_text(&task.notes),
            error: None,
        }
    }

    /// Builds the updated task from the form, or an error message.
    ///
    /// The due field: empty clears the date; unchanged keeps the task's date
    /// without parsing; anything else goes through `parse_date`.
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
        let due_input = self.due.trim();
        let due_on = if due_input.is_empty() {
            None
        } else if due_input == self.original_due.trim() {
            task.due_on
        } else {
            match parse_date(due_input, today) {
                Some(d) => Some(d),
                None => {
                    return Err(format!(
                        "Can't read \"{due_input}\" as a date. Try fri, tomorrow, nov 20, +3d."
                    ));
                }
            }
        };
        let project = self.project.trim().trim_start_matches('#').trim();
        let notes = self.notes.text();
        Ok(Task {
            title: title.to_string(),
            project: (!project.is_empty()).then(|| project.to_string()),
            due_on,
            notes: notes.trim_end().to_string(),
            ..task.clone()
        })
    }
}

impl TasksView {
    pub fn new() -> Self {
        Self::default()
    }

    /// A task that focuses the quick-add input (called at launch).
    pub fn focus_quick_add() -> iced::Task<Message> {
        operation::focus(QUICK_ADD_ID)
    }

    /// `tasks` is every task (done or not); `today` is the local date.
    pub fn update(&mut self, message: Message, tasks: &[Task], today: Date) -> Action {
        match message {
            Message::QuickAddChanged(s) => {
                self.quick_add = s;
                Action::None
            }
            Message::QuickAddSubmitted => {
                if self.quick_add.trim().is_empty() {
                    return Action::None;
                }
                let parsed = parse::parse_quick_add(&self.quick_add, today);
                if parsed.title.trim().is_empty() {
                    return Action::None;
                }
                self.quick_add.clear();
                Action::Add(NewTask {
                    title: parsed.title,
                    notes: String::new(),
                    project: parsed.project,
                    due_on: parsed.due_on,
                })
            }
            Message::Toggle(id, done) => Action::SetDone(id, done),
            Message::Expand(id) => {
                if self.editing.as_ref().is_some_and(|f| f.id == id) {
                    self.editing = None;
                } else if let Some(task) = tasks.iter().find(|t| t.id == id) {
                    self.editing = Some(EditForm::from_task(task));
                }
                Action::None
            }
            Message::EditTitle(s) => self.edit(|f| f.title = s),
            Message::EditProject(s) => self.edit(|f| f.project = s),
            Message::EditDue(s) => self.edit(|f| {
                f.due = s;
                f.error = None;
            }),
            Message::EditNotes(action) => self.edit(|f| f.notes.perform(action)),
            Message::Save => {
                let Some(form) = self.editing.as_mut() else {
                    return Action::None;
                };
                let Some(task) = tasks.iter().find(|t| t.id == form.id) else {
                    self.editing = None;
                    return Action::None;
                };
                match form.apply(task, today, parse::parse_date) {
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
        }
    }

    fn edit(&mut self, f: impl FnOnce(&mut EditForm)) -> Action {
        if let Some(form) = self.editing.as_mut() {
            f(form);
        }
        Action::None
    }

    pub fn view<'a>(&'a self, tasks: &'a [Task], today: Date) -> Element<'a, Message> {
        let quick_add = text_input(
            "Add a task — e.g. Write Q4 report fri #work",
            &self.quick_add,
        )
        .id(QUICK_ADD_ID)
        .on_input(Message::QuickAddChanged)
        .on_submit(Message::QuickAddSubmitted)
        .padding([10, 14])
        .size(theme::TEXT_BODY + 1.0)
        .style(theme::input);

        let hint = text("Dates: today · tomorrow · fri · nov 20 · +3d     Project: #name")
            .size(theme::TEXT_SMALL)
            .style(theme::muted);

        let mut content = column![
            theme::title("Tasks"),
            column![quick_add, hint].spacing(theme::SPACING / 2.0),
        ]
        .spacing(theme::SPACING * 1.5);

        let mut any_open = false;
        for section in Section::OPEN {
            let items = sorted_section(tasks, section, today);
            if items.is_empty() {
                continue;
            }
            any_open = true;
            content = content.push(self.section(section, items, today));
        }
        if !any_open {
            content = content.push(
                text("Nothing to do. Add something above.")
                    .size(theme::TEXT_BODY)
                    .style(theme::muted),
            );
        }

        let done = sorted_section(tasks, Section::Done, today);
        if !done.is_empty() {
            let arrow = if self.show_done { "▾" } else { "▸" };
            let toggle =
                button(text(format!("{arrow}  Done  {}", done.len())).size(theme::TEXT_SMALL))
                    .padding([4, 0])
                    .style(theme::text_button)
                    .on_press(Message::ToggleShowDone);
            let mut group = column![toggle].spacing(2);
            if self.show_done {
                for task in done {
                    group = group.push(self.task(task, today));
                }
            }
            content = content.push(group);
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

    fn section<'a>(
        &'a self,
        section: Section,
        items: Vec<&'a Task>,
        today: Date,
    ) -> Element<'a, Message> {
        let heading = row![
            text(section.label())
                .size(theme::TEXT_SMALL)
                .style(if section == Section::Overdue {
                    theme::danger
                } else {
                    theme::muted
                }),
            text(items.len().to_string())
                .size(theme::TEXT_SMALL)
                .style(theme::muted),
        ]
        .spacing(theme::SPACING / 2.0);
        let rows = Column::with_children(items.into_iter().map(|t| self.task(t, today))).spacing(2);
        column![heading, rows].spacing(theme::SPACING / 2.0).into()
    }

    fn task<'a>(&'a self, task: &'a Task, today: Date) -> Element<'a, Message> {
        match &self.editing {
            Some(form) if form.id == task.id => edit_card(task, form),
            _ => task_row(task, today),
        }
    }
}

fn task_row<'a>(task: &'a Task, today: Date) -> Element<'a, Message> {
    let id = task.id;
    let done = task.is_done();

    let mut title = text(&task.title).size(theme::TEXT_BODY);
    if done {
        title = title.style(theme::muted);
    }
    let mut line = row![title].spacing(theme::SPACING).align_y(Center);
    if let Some(project) = &task.project {
        line = line.push(project_chip(project));
    }
    line = line.push(Space::new().width(Fill));
    if let Some(due) = task.due_on {
        let label = text(due_label(due, today)).size(theme::TEXT_SMALL);
        line = line.push(if is_overdue(task, today) {
            label.style(theme::danger)
        } else {
            label.style(theme::muted)
        });
    }

    row![
        checkbox(done).on_toggle(move |b| Message::Toggle(id, b)),
        button(line)
            .width(Fill)
            .padding([8, 10])
            .style(theme::row_button)
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
    .into()
}

fn edit_card<'a>(task: &'a Task, form: &'a EditForm) -> Element<'a, Message> {
    let id = task.id;
    let title = text_input("Title", &form.title)
        .on_input(Message::EditTitle)
        .on_submit(Message::Save)
        .padding([8, 10])
        .size(theme::TEXT_BODY)
        .style(theme::input);
    let project = text_input("none", &form.project)
        .on_input(Message::EditProject)
        .on_submit(Message::Save)
        .padding([8, 10])
        .size(theme::TEXT_BODY)
        .style(theme::input);
    let due = text_input("none — try fri, nov 20, +3d", &form.due)
        .on_input(Message::EditDue)
        .on_submit(Message::Save)
        .padding([8, 10])
        .size(theme::TEXT_BODY)
        .style(theme::input);
    let notes = text_editor(&form.notes)
        .placeholder("Notes")
        .on_action(Message::EditNotes)
        .height(110)
        .padding(10)
        .size(theme::TEXT_BODY)
        .style(theme::editor);

    let mut body = column![
        row![
            checkbox(task.is_done()).on_toggle(move |b| Message::Toggle(id, b)),
            title
        ]
        .spacing(theme::SPACING / 2.0)
        .align_y(Center),
        row![labelled("Project", project), labelled("Due", due)].spacing(theme::SPACING),
    ]
    .spacing(theme::SPACING);

    if let Some(error) = &form.error {
        body = body.push(text(error).size(theme::TEXT_SMALL).style(theme::danger));
    }

    body = body.push(labelled("Notes", notes));
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

    container(body)
        .padding(theme::SPACING * 1.25)
        .width(Fill)
        .style(theme::card)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fortnight_core::domain::TaskStatus;
    use jiff::Timestamp;
    use jiff::civil::date;

    /// Monday 16 November 2026.
    fn today() -> Date {
        date(2026, 11, 16)
    }

    fn task(id: TaskId, title: &str, due: Option<Date>) -> Task {
        Task {
            id,
            title: title.to_string(),
            notes: String::new(),
            status: TaskStatus::Todo,
            project: None,
            due_on: due,
            position: id as f64,
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::UNIX_EPOCH,
            completed_at: None,
        }
    }

    fn no_parse(_: &str, _: Date) -> Option<Date> {
        panic!("parse_date must not be called")
    }

    #[test]
    fn section_of_table() {
        let cases = [
            (Some(date(2026, 11, 1)), Section::Overdue),
            (Some(date(2026, 11, 15)), Section::Overdue),
            (Some(date(2026, 11, 16)), Section::Today),
            (Some(date(2026, 11, 17)), Section::Next7Days),
            (Some(date(2026, 11, 23)), Section::Next7Days),
            (Some(date(2026, 11, 24)), Section::Later),
            (Some(date(2027, 3, 1)), Section::Later),
            (None, Section::NoDate),
        ];
        for (due, want) in cases {
            assert_eq!(section_of(&task(1, "t", due), today()), want, "{due:?}");
        }
        let mut done = task(1, "t", Some(date(2026, 11, 1)));
        done.status = TaskStatus::Done;
        assert_eq!(section_of(&done, today()), Section::Done);
        done.status = TaskStatus::Dropped;
        assert_eq!(section_of(&done, today()), Section::Done);
        let mut doing = task(1, "t", Some(today()));
        doing.status = TaskStatus::Doing;
        assert_eq!(section_of(&doing, today()), Section::Today);
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
            (date(2026, 12, 5), "Dec 5"),
            (date(2027, 1, 4), "Jan 4, 2027"),
        ];
        for (due, want) in cases {
            assert_eq!(due_label(due, today()), want, "{due}");
        }
    }

    #[test]
    fn sections_sort_by_due_then_position() {
        let mut a = task(1, "a", Some(date(2026, 11, 20)));
        a.position = 5.0;
        let mut b = task(2, "b", Some(date(2026, 11, 18)));
        b.position = 9.0;
        let mut c = task(3, "c", Some(date(2026, 11, 20)));
        c.position = 1.0;
        let tasks = vec![a, b, c];
        let ids: Vec<_> = sorted_section(&tasks, Section::Next7Days, today())
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![2, 3, 1]);
    }

    #[test]
    fn toggling_returns_set_done() {
        let tasks = vec![task(7, "t", None)];
        let mut view = TasksView::new();
        assert_eq!(
            view.update(Message::Toggle(7, true), &tasks, today()),
            Action::SetDone(7, true)
        );
        assert_eq!(
            view.update(Message::Toggle(7, false), &tasks, today()),
            Action::SetDone(7, false)
        );
    }

    #[test]
    fn delete_returns_delete_and_closes() {
        let tasks = vec![task(3, "t", None)];
        let mut view = TasksView::new();
        view.update(Message::Expand(3), &tasks, today());
        assert!(view.editing.is_some());
        assert_eq!(
            view.update(Message::Delete, &tasks, today()),
            Action::Delete(3)
        );
        assert!(view.editing.is_none());
    }

    #[test]
    fn save_with_unchanged_due_returns_update_with_new_title() {
        // The due field is untouched, so parse_date (a stub here) is not called.
        let tasks = vec![task(4, "Old", Some(date(2026, 11, 20)))];
        let mut view = TasksView::new();
        view.update(Message::Expand(4), &tasks, today());
        view.update(Message::EditTitle("New title".into()), &tasks, today());
        let Action::Update(updated) = view.update(Message::Save, &tasks, today()) else {
            panic!("expected Update");
        };
        assert_eq!(updated.title, "New title");
        assert_eq!(updated.due_on, Some(date(2026, 11, 20)));
        assert_eq!(updated.id, 4);
        assert!(view.editing.is_none());
    }

    #[test]
    fn empty_quick_add_returns_none() {
        let mut view = TasksView::new();
        assert_eq!(
            view.update(Message::QuickAddSubmitted, &[], today()),
            Action::None
        );
        view.update(Message::QuickAddChanged("   ".into()), &[], today());
        assert_eq!(
            view.update(Message::QuickAddSubmitted, &[], today()),
            Action::None
        );
    }

    #[test]
    fn only_one_row_expanded_and_click_again_collapses() {
        let tasks = vec![task(1, "a", None), task(2, "b", None)];
        let mut view = TasksView::new();
        view.update(Message::Expand(1), &tasks, today());
        view.update(Message::Expand(2), &tasks, today());
        assert_eq!(view.editing.as_ref().map(|f| f.id), Some(2));
        view.update(Message::Expand(2), &tasks, today());
        assert!(view.editing.is_none());
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
    fn invalid_due_shows_error_and_saves_nothing() {
        let t = task(1, "Title", None);
        let mut form = EditForm::from_task(&t);
        form.due = "blah".into();
        let err = form.apply(&t, today(), |_, _| None).unwrap_err();
        assert!(err.contains("blah"));
    }
}
