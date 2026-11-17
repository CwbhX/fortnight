//! Visual check for the Review view with fixture data.
//!
//! `cargo run -p fortnight-app --example review_preview`
//!
//! Env: `PREVIEW_STATE=ready|empty|failed|editing|generating|blocked`
//! (default `ready`); `PREVIEW_DARK=1` forces the dark theme;
//! `FORTNIGHT_SCREENSHOT=<path>` screenshots after two seconds and exits.
//! Actions are simulated: Generate fakes a report after a second, SaveEdit
//! stores the edit, ShowWeek switches to an empty week, Copy prints.

use fortnight_app::theme::theme_for;
use fortnight_app::views::review::{Action, Message as ReviewMessage, ReviewData, ReviewView};
use fortnight_core::domain::{Report, ReportKind, ReportStatus, WeekRef};
use iced::{Element, Task as Command, Theme};
use jiff::Timestamp;
use jiff::civil::date;

pub fn main() -> iced::Result {
    iced::application(Preview::new, Preview::update, Preview::view)
        .title("Review preview")
        .theme(Preview::theme)
        .window_size((1180.0, 820.0))
        .run()
}

struct Preview {
    view: ReviewView,
    data: ReviewData,
    dark: bool,
}

#[derive(Debug, Clone)]
enum Message {
    Review(ReviewMessage),
    Generated,
    Shoot,
    Shot(iced::window::Screenshot),
}

impl Preview {
    fn new() -> (Self, Command<Message>) {
        let state = std::env::var("PREVIEW_STATE").unwrap_or_else(|_| "ready".into());
        let mut data = fixture(WeekRef::new(date(2026, 11, 16)));
        let mut start = Command::none();
        match state.as_str() {
            "empty" => data.report = None,
            "generating" => data.generating = true,
            "blocked" => {
                data.report = None;
                data.blocked = Some("Unlock to generate".into());
            }
            "failed" => {
                let r = data.report.as_mut().unwrap();
                r.status = ReportStatus::Failed;
                r.content_md = None;
                r.error = Some("The endpoint returned 401 Unauthorized: check the API key.".into());
            }
            // Open the editor after the first frames, like a real click.
            "editing" => {
                start = Command::perform(
                    tokio::time::sleep(std::time::Duration::from_millis(800)),
                    |_| Message::Review(ReviewMessage::StartEdit),
                )
            }
            _ => {}
        }
        let mut view = ReviewView::new();
        view.load(data.clone());
        let preview = Preview {
            view,
            data,
            dark: std::env::var_os("PREVIEW_DARK").is_some(),
        };
        (preview, Command::batch([start, shoot_after_delay()]))
    }

    fn theme(&self) -> Option<Theme> {
        Some(theme_for(self.dark))
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        let mut command = Command::none();
        match message {
            Message::Review(m) => match self.view.update(m) {
                Action::None => {}
                Action::ShowWeek(week) => {
                    self.data = fixture(week);
                    self.data.report = None;
                }
                Action::Generate(_) => {
                    self.data.generating = true;
                    command = Command::perform(
                        tokio::time::sleep(std::time::Duration::from_secs(1)),
                        |_| Message::Generated,
                    );
                }
                Action::SaveEdit(id, text) => {
                    if let Some(r) = self.data.report.as_mut().filter(|r| r.id == id) {
                        r.edited_md = Some(text);
                    }
                }
                Action::Copy(text) => println!("copy:\n{text}"),
            },
            Message::Generated => {
                let id = self.data.report.as_ref().map_or(1, |r| r.id + 1);
                self.data.generating = false;
                self.data.report = Some(report(id, self.data.week));
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
        self.view.view().map(Message::Review)
    }
}

fn fixture(week: WeekRef) -> ReviewData {
    let label = format!(
        "Week {} · {}–{}",
        week.start.iso_week_date().week(),
        week.start.strftime("%b %-d"),
        week.end().strftime("%-d"),
    );
    ReviewData {
        week,
        week_label: label,
        facts_md: FACTS.into(),
        report: Some(report(1, week)),
        generating: false,
        blocked: None,
    }
}

fn report(id: i64, week: WeekRef) -> Report {
    Report {
        id,
        kind: ReportKind::Summary,
        week_start: week.start,
        status: ReportStatus::Ready,
        digest_json: "{}".into(),
        content_md: Some(SUMMARY.into()),
        edited_md: None,
        model: Some("gpt-4o-mini".into()),
        endpoint_host: Some("api.openai.com".into()),
        prompt_version: 1,
        error: None,
        created_at: "2026-11-20T16:02:00Z"
            .parse()
            .unwrap_or(Timestamp::UNIX_EPOCH),
    }
}

const FACTS: &str = "\
**Completed (4)**
- Write Q4 report — *work*, Tue
- Review Sam's deploy PR — *work*, Wed
- Send invoice to Acme — *admin*, Thu
- Draft hiring scorecard — *work*, Fri

**Added (3)**
- Plan team offsite agenda — *work*
- Renew passport
- Fix the bike light

**Slipped (1)**
- Book flights for December: Nov 18 → Nov 25

**By project:** work 3, admin 1
";

const SUMMARY: &str = "\
## Highlights
- Delivered the **Q4 report** on time, including the churn analysis.

## Completed
- *Work:* Q4 report; reviewed Sam's deploy PR; drafted the hiring scorecard.
- *Admin:* sent the Acme invoice.

## In progress
- Team offsite agenda: first draft started.

## Risks and blockers
- December flights slipped a week; prices may rise.

## Next week
- Finalise the offsite agenda and share it by Wednesday.
";

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
