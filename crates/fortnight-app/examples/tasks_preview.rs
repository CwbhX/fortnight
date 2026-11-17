//! Visual check for the Tasks view with in-memory fixture tasks.
//!
//! `cargo run -p fortnight-app --example tasks_preview`
//!
//! Env: `PREVIEW_DARK=1` forces the dark theme; `PREVIEW_EXPAND=<id>` opens
//! that row's edit form; `PREVIEW_SHOW_DONE=1` opens the Done section;
//! `FORTNIGHT_SCREENSHOT=<path>` screenshots after two seconds and exits.
//! Quick-add and Save call the parser, which may still be a stub.

use fortnight_app::theme::theme_for;
use fortnight_app::views::tasks::{Action, Message as TasksMessage, TasksView};
use fortnight_core::domain::{Task, TaskStatus};
use iced::{Element, Task as Command, Theme};
use jiff::Timestamp;
use jiff::civil::{Date, date};

pub fn main() -> iced::Result {
    iced::application(Preview::new, Preview::update, Preview::view)
        .title("Tasks preview")
        .theme(Preview::theme)
        .window_size((900.0, 760.0))
        .run()
}

struct Preview {
    view: TasksView,
    tasks: Vec<Task>,
    today: Date,
    dark: bool,
}

#[derive(Debug, Clone)]
enum Message {
    Tasks(TasksMessage),
    Shoot,
    Shot(iced::window::Screenshot),
}

impl Preview {
    fn new() -> (Self, Command<Message>) {
        let today = date(2026, 11, 16);
        let tasks = fixtures(today);
        let mut view = TasksView::new();
        // Expand after the first frames, like a real click (a text_editor
        // created before the window is up sometimes draws empty).
        let expand = match std::env::var("PREVIEW_EXPAND")
            .ok()
            .and_then(|s| s.parse().ok())
        {
            Some(id) => Command::perform(
                tokio::time::sleep(std::time::Duration::from_millis(800)),
                move |_| Message::Tasks(TasksMessage::Expand(id)),
            ),
            None => Command::none(),
        };
        if std::env::var_os("PREVIEW_SHOW_DONE").is_some() {
            view.update(TasksMessage::ToggleShowDone, &tasks, today);
        }
        let preview = Preview {
            view,
            tasks,
            today,
            dark: std::env::var_os("PREVIEW_DARK").is_some(),
        };
        let focus = TasksView::focus_quick_add().map(Message::Tasks);
        (
            preview,
            Command::batch([focus, expand, shoot_after_delay()]),
        )
    }

    fn theme(&self) -> Option<Theme> {
        Some(theme_for(self.dark))
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Tasks(m) => match self.view.update(m, &self.tasks, self.today) {
                Action::None => {}
                Action::Add(new) => {
                    let id = self.tasks.iter().map(|t| t.id).max().unwrap_or(0) + 1;
                    let mut t = task(id, &new.title, new.due_on, new.project.as_deref());
                    t.notes = new.notes;
                    self.tasks.push(t);
                }
                Action::SetDone(id, done) => {
                    if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
                        t.status = if done {
                            TaskStatus::Done
                        } else {
                            TaskStatus::Todo
                        };
                        t.completed_at = done.then(Timestamp::now);
                    }
                }
                Action::Update(updated) => {
                    if let Some(t) = self.tasks.iter_mut().find(|t| t.id == updated.id) {
                        *t = updated;
                    }
                }
                Action::Delete(id) => self.tasks.retain(|t| t.id != id),
            },
            Message::Shoot => {
                return iced::window::latest()
                    .and_then(iced::window::screenshot)
                    .map(Message::Shot);
            }
            Message::Shot(shot) => return save_screenshot(shot),
        }
        Command::none()
    }

    fn view(&self) -> Element<'_, Message> {
        self.view.view(&self.tasks, self.today).map(Message::Tasks)
    }
}

fn task(id: i64, title: &str, due: Option<Date>, project: Option<&str>) -> Task {
    Task {
        id,
        title: title.to_string(),
        notes: String::new(),
        status: TaskStatus::Todo,
        project: project.map(str::to_string),
        due_on: due,
        position: id as f64,
        created_at: Timestamp::UNIX_EPOCH,
        updated_at: Timestamp::UNIX_EPOCH,
        completed_at: None,
    }
}

fn fixtures(today: Date) -> Vec<Task> {
    use jiff::ToSpan;
    let d = |n: i64| Some(today.saturating_add(n.days()));
    let mut tasks = vec![
        task(1, "Send invoice to Acme", d(-2), Some("admin")),
        task(2, "Write Q4 report", d(0), Some("work")),
        task(3, "Call the dentist", d(0), None),
        task(4, "Review Sam's deploy PR", d(1), Some("work")),
        task(5, "Plan team offsite agenda", d(4), Some("work")),
        task(6, "Renew passport", d(19), None),
        task(
            7,
            "Read 'Designing Data-Intensive Applications'",
            None,
            Some("reading"),
        ),
        task(8, "Fix the bike light", None, None),
        task(9, "Book flights for December", d(-1), Some("home")),
        task(10, "Draft hiring scorecard", d(-3), Some("work")),
    ];
    tasks[1].notes = "Pull numbers from the dashboard.\nAsk Priya for the churn chart.".into();
    for t in tasks.iter_mut().filter(|t| t.id >= 9) {
        t.status = TaskStatus::Done;
        t.completed_at = Some(Timestamp::UNIX_EPOCH);
    }
    tasks
}

fn shoot_after_delay() -> Command<Message> {
    if std::env::var_os("FORTNIGHT_SCREENSHOT").is_none() {
        return Command::none();
    }
    Command::perform(
        tokio::time::sleep(std::time::Duration::from_secs(2)),
        |_| Message::Shoot,
    )
}

fn save_screenshot(shot: iced::window::Screenshot) -> Command<Message> {
    if let Some(path) = std::env::var_os("FORTNIGHT_SCREENSHOT") {
        let mut bytes = format!("{} {}\n", shot.size.width, shot.size.height).into_bytes();
        bytes.extend_from_slice(&shot.rgba);
        if let Err(e) = std::fs::write(&path, bytes) {
            eprintln!("screenshot: {e}");
        }
    }
    iced::exit()
}
