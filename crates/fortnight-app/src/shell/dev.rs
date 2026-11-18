//! Developer hooks: the forced theme (`FORTNIGHT_THEME`), the self
//! screenshot (`FORTNIGHT_SCREENSHOT`), the `--data-dir` flag, the start
//! page (`FORTNIGHT_PAGE`) and held animations (`FORTNIGHT_MOTION`).

use crate::Message;
use fortnight_app::motion;
use fortnight_app::top_bar::Tab;
use iced::Task;
use std::path::PathBuf;
use std::time::Duration;

/// Dev aid: `FORTNIGHT_THEME=light|dark` overrides the system theme (for
/// screenshots of both).
pub(crate) fn forced_theme() -> Option<bool> {
    match std::env::var("FORTNIGHT_THEME").as_deref() {
        Ok("dark") => Some(true),
        Ok("light") => Some(false),
        _ => None,
    }
}

/// Sets up the animations: off for the self screenshot (it shows each
/// thing in its final place) and when the system asks for reduced motion.
/// Dev aid: `FORTNIGHT_MOTION=<0 to 1>` holds every animation that far
/// along instead, for a screenshot of one mid-way.
pub(crate) fn set_up_motion() {
    let held = std::env::var("FORTNIGHT_MOTION")
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok());
    if let Some(progress) = held {
        motion::hold_at(progress);
    } else if std::env::var_os("FORTNIGHT_SCREENSHOT").is_some() || motion::system_reduces_motion()
    {
        motion::set_enabled(false);
    }
}

/// `--data-dir <path>` or `--data-dir=<path>`.
pub(crate) fn data_dir_flag() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--data-dir" {
            return args.next().map(PathBuf::from);
        }
        if let Some(v) = arg.to_str().and_then(|s| s.strip_prefix("--data-dir=")) {
            return Some(PathBuf::from(v));
        }
    }
    None
}

/// Dev aid: with `FORTNIGHT_SCREENSHOT=<path>`, the app screenshots its own
/// window after two seconds, writes `<width> <height>\n` + raw RGBA to
/// `<path>`, and exits. Lets a headless agent check what renders.
pub(crate) fn shoot_after_delay() -> Task<Message> {
    if std::env::var_os("FORTNIGHT_SCREENSHOT").is_none() {
        return Task::none();
    }
    // `FORTNIGHT_SCREENSHOT_DELAY` (seconds) waits longer, e.g. for the
    // Ollama check.
    let secs = std::env::var("FORTNIGHT_SCREENSHOT_DELAY")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(2);
    Task::perform(tokio::time::sleep(Duration::from_secs(secs)), |_| {
        Message::Shoot
    })
}

pub(crate) fn save_screenshot(shot: iced::window::Screenshot) -> Task<Message> {
    if let Some(path) = std::env::var_os("FORTNIGHT_SCREENSHOT") {
        let mut bytes = format!("{} {}\n", shot.size.width, shot.size.height).into_bytes();
        bytes.extend_from_slice(&shot.rgba);
        if let Err(e) = std::fs::write(&path, bytes) {
            eprintln!("screenshot: {e}");
        }
    }
    iced::exit()
}

/// The tab to start on, and whether Settings starts open
/// (`FORTNIGHT_PAGE=today|week|month|settings`).
pub(crate) fn start_page() -> (Tab, bool) {
    match std::env::var("FORTNIGHT_PAGE").as_deref() {
        Ok("week") => (Tab::Week, false),
        Ok("month") => (Tab::Month, false),
        Ok("settings") => (Tab::Today, true),
        _ => (Tab::Today, false),
    }
}
