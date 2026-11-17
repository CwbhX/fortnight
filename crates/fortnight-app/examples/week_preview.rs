//! Visual check for the This-week view with in-memory fixture data.
//!
//! `cargo run -p fortnight-app --example week_preview`
//!
//! Env: `PREVIEW_DARK=1` forces the dark theme; `PREVIEW_STATE` picks the
//! Advice card state: `ready` (default), `empty`, `generating`, `failed` or
//! `blocked`; `PREVIEW_FACTS=1` opens the facts; `FORTNIGHT_SCREENSHOT=<path>`
//! screenshots after two seconds and exits.
//!
//! Generate pretends to finish after a second with fixture advice.

use fortnight_app::theme::theme_for;
use fortnight_app::views::week::{Action, Message as WeekMessage, WeekData, WeekView};
use fortnight_core::domain::{Report, ReportKind, ReportStatus, Task, TaskStatus, WeekRef};
use iced::{Element, Task as Command, Theme};
use jiff::civil::{Date, date};
use jiff::{Timestamp, ToSpan};

pub fn main() -> iced::Result {
    iced::application(Preview::new, Preview::update, Preview::view)
        .title("This week preview")
        .theme(Preview::theme)
        .window_size((1000.0, 760.0))
        .run()
}

struct Preview {
    view: WeekView,
    data: WeekData,
    dark: bool,
}

#[derive(Debug, Clone)]
enum Message {
    Week(WeekMessage),
    Generated,
    Shoot,
    Shot(iced::window::Screenshot),
}

impl Preview {
    fn new() -> (Self, Command<Message>) {
        let today = date(2026, 11, 18);
        let week = WeekRef::new(date(2026, 11, 16));
        let state = std::env::var("PREVIEW_STATE").unwrap_or_else(|_| "ready".into());
        let mut data = WeekData {
            week,
            week_label: "Week 47 · 16–22 November".into(),
            today,
            tasks: fixtures(week.start),
            facts_md: FACTS.into(),
            advice: None,
            generating: false,
            blocked: None,
        };
        match state.as_str() {
            "empty" => {}
            "generating" => data.generating = true,
            "failed" => {
                let mut r = advice(week.start);
                r.status = ReportStatus::Failed;
                r.content_md = None;
                r.error = Some("HTTP 401: invalid API key".into());
                data.advice = Some(r);
            }
            "blocked" => {
                data.blocked = Some("Unlock your API key to generate advice.".into());
            }
            _ => data.advice = Some(advice(week.start)),
        }

        let mut view = WeekView::new();
        view.load(data.clone());
        if std::env::var_os("PREVIEW_FACTS").is_some() {
            view.update(WeekMessage::ToggleFacts);
        }
        let preview = Preview {
            view,
            data,
            dark: std::env::var_os("PREVIEW_DARK").is_some(),
        };
        (preview, shoot_after_delay())
    }

    fn theme(&self) -> Option<Theme> {
        Some(theme_for(self.dark))
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        let mut command = Command::none();
        match message {
            Message::Week(m) => match self.view.update(m) {
                Action::None => {}
                Action::Generate => {
                    self.data.generating = true;
                    command = Command::perform(
                        tokio::time::sleep(std::time::Duration::from_secs(1)),
                        |_| Message::Generated,
                    );
                }
                Action::SetDone(id, done) => {
                    if let Some(t) = self.data.tasks.iter_mut().find(|t| t.id == id) {
                        t.status = if done {
                            TaskStatus::Done
                        } else {
                            TaskStatus::Todo
                        };
                        t.completed_at = done.then(Timestamp::now);
                    }
                }
            },
            Message::Generated => {
                self.data.generating = false;
                self.data.advice = Some(advice(self.data.week.start));
            }
            Message::Shoot => {
                return iced::window::latest()
                    .and_then(iced::window::screenshot)
                    .map(Message::Shot);
            }
            Message::Shot(shot) => return save_screenshot(shot),
        }
        // Like main.rs: load after every message.
        self.view.load(self.data.clone());
        command
    }

    fn view(&self) -> Element<'_, Message> {
        self.view.view().map(Message::Week)
    }
}

const FACTS: &str = "\
**Today**: Wednesday 18 November. Remaining working days: Wed, Thu, Fri.

- Open tasks: 7, of which 2 overdue.
- Due this week: 4.
- No due date: 1.
";

const ADVICE: &str = "\
**Focus**: get the Q4 report out by Friday; everything else bends around it.

**At risk**
- *Send invoice to Acme* is two days overdue — do it first thing today.
- *Write Q4 report* is due Friday and still needs numbers from the dashboard.

**Suggested order**
1. Wed: invoice, then start the report outline.
2. Thu: review Sam's PR in the morning, report draft after.
3. Fri: finish the report; offsite agenda if time allows.

**Cut or renegotiate**
- Move *Plan team offsite agenda* to next week.

**Gaps**
- *Fix the bike light* has no date.
";

fn advice(week_start: Date) -> Report {
    Report {
        id: 1,
        kind: ReportKind::Plan,
        week_start,
        status: ReportStatus::Ready,
        digest_json: "{}".into(),
        content_md: Some(ADVICE.into()),
        edited_md: None,
        model: Some("gpt-4.1-mini".into()),
        endpoint_host: Some("api.openai.com".into()),
        prompt_version: 1,
        error: None,
        created_at: "2026-11-16T08:00:00Z".parse().unwrap(),
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

/// Days relative to the week's Monday; today is Wednesday (+2).
fn fixtures(monday: Date) -> Vec<Task> {
    let d = |n: i64| Some(monday.saturating_add(n.days()));
    let mut tasks = vec![
        task(1, "Send invoice to Acme", d(0), Some("admin")),
        task(2, "Write Q4 report", d(4), Some("work")),
        task(3, "Call the dentist", d(2), None),
        task(4, "Review Sam's deploy PR", d(3), Some("work")),
        task(5, "Plan team offsite agenda", d(4), Some("work")),
        task(6, "Renew passport", d(19), None),
        task(7, "Fix the bike light", None, None),
        task(8, "Book flights for December", d(2), Some("home")),
        task(9, "Draft hiring scorecard", d(-3), Some("work")),
    ];
    tasks[7].status = TaskStatus::Done;
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
