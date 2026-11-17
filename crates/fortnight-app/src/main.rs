//! Fortnight. Step 1: a bare window proving iced works (input, list,
//! Markdown). The orchestrator replaces this with the real wiring in step 3.

use iced::widget::{checkbox, column, markdown, scrollable, text, text_input};
use iced::{Element, Fill, Task, Theme};

pub fn main() -> iced::Result {
    iced::application(Spike::new, Spike::update, Spike::view)
        .title("Fortnight")
        .theme(Spike::theme)
        .window_size((900.0, 700.0))
        .run()
}

struct Spike {
    input: String,
    items: Vec<(String, bool)>,
    markdown: markdown::Content,
}

#[derive(Debug, Clone)]
enum Message {
    InputChanged(String),
    Submit,
    Toggle(usize, bool),
    Link,
    Shoot,
    Shot(iced::window::Screenshot),
}

impl Spike {
    fn new() -> (Self, Task<Message>) {
        (
            Spike {
                input: String::new(),
                items: vec![("Write Q4 report".into(), false)],
                markdown: markdown::Content::parse(
                    "## Highlights\n\n- Shipped the **spike**\n- [ ] A task list item\n\n`code`",
                ),
            },
            shoot_after_delay(),
        )
    }

    fn theme(&self) -> Option<Theme> {
        None
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Shoot => {
                return iced::window::latest()
                    .and_then(iced::window::screenshot)
                    .map(Message::Shot);
            }
            Message::Shot(shot) => return save_screenshot(shot),
            Message::InputChanged(s) => self.input = s,
            Message::Submit => {
                if !self.input.trim().is_empty() {
                    self.items.push((std::mem::take(&mut self.input), false));
                }
            }
            Message::Toggle(i, done) => self.items[i].1 = done,
            Message::Link => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let input = text_input("Add a task…", &self.input)
            .on_input(Message::InputChanged)
            .on_submit(Message::Submit)
            .padding(10);
        let list = column(self.items.iter().enumerate().map(|(i, (title, done))| {
            checkbox(*done)
                .label(title.as_str())
                .on_toggle(move |b| Message::Toggle(i, b))
                .into()
        }))
        .spacing(8);
        let md = markdown::view(self.markdown.items(), Theme::Light).map(|_uri| Message::Link);
        scrollable(
            column![text("Fortnight").size(28), input, list, md]
                .spacing(16)
                .padding(24)
                .width(Fill),
        )
        .into()
    }
}

/// Dev aid: with `FORTNIGHT_SCREENSHOT=<path>`, the app screenshots its own
/// window after two seconds, writes `<width> <height>\n` + raw RGBA to
/// `<path>`, and exits. Lets a headless agent check what renders.
fn shoot_after_delay() -> Task<Message> {
    if std::env::var_os("FORTNIGHT_SCREENSHOT").is_none() {
        return Task::none();
    }
    Task::perform(
        tokio::time::sleep(std::time::Duration::from_secs(2)),
        |_| Message::Shoot,
    )
}

fn save_screenshot(shot: iced::window::Screenshot) -> Task<Message> {
    if let Some(path) = std::env::var_os("FORTNIGHT_SCREENSHOT") {
        let mut bytes = format!("{} {}\n", shot.size.width, shot.size.height).into_bytes();
        bytes.extend_from_slice(&shot.rgba);
        if let Err(e) = std::fs::write(&path, bytes) {
            eprintln!("screenshot: {e}");
        }
    }
    iced::exit()
}
