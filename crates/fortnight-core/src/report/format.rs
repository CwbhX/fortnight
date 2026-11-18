//! Text helpers shared by the facts, their Markdown and the prompts:
//! dates, weekdays, hours, titles and notes.

use super::summary::*;
use jiff::civil::{Date, Weekday};
use serde::{Deserialize, Deserializer};
use std::fmt::Write as _;

/// Task notes longer than this (in characters) are cut and end in "…".
pub(super) const NOTES_MAX_CHARS: usize = 280;

/// The same for log lines. A day's notes are one log line, so they get
/// more room than a task's notes.
pub(super) const LOG_MAX_CHARS: usize = 2000;

/// Minutes as hours, rounded to one decimal (90 → 1.5).
pub(super) fn hours(minutes: u32) -> f64 {
    (f64::from(minutes) / 6.0).round() / 10.0
}

/// Minutes as a short duration, for figures (not for editing): under an
/// hour in minutes, otherwise hours to one decimal. 30 → "30m",
/// 60 → "1h", 90 → "1.5h", 100 → "1.7h", 480 → "8h".
pub fn fmt_hours(minutes: u32) -> String {
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let tenths = (u64::from(minutes) * 10 + 30) / 60;
    if tenths % 10 == 0 {
        format!("{}h", tenths / 10)
    } else {
        format!("{}.{}h", tenths / 10, tenths % 10)
    }
}

/// "Monday".
pub fn weekday_name(day: Weekday) -> &'static str {
    match day {
        Weekday::Monday => "Monday",
        Weekday::Tuesday => "Tuesday",
        Weekday::Wednesday => "Wednesday",
        Weekday::Thursday => "Thursday",
        Weekday::Friday => "Friday",
        Weekday::Saturday => "Saturday",
        Weekday::Sunday => "Sunday",
    }
}

/// A weekday name from `weekday_name`. An alias, not `&'static str`
/// written out, because serde's derive borrows any field spelled `&str`
/// from the input, which would only accept `'static` input.
pub type WeekdayName = &'static str;

/// Reads a weekday name as written by `weekday_name`, back into the same
/// static string.
pub(super) fn static_weekday<'de, D: Deserializer<'de>>(d: D) -> Result<WeekdayName, D::Error> {
    let name = String::deserialize(d)?;
    WEEKDAYS
        .into_iter()
        .map(weekday_name)
        .find(|w| *w == name)
        .ok_or_else(|| serde::de::Error::custom(format!("not a weekday: {name:?}")))
}

pub(super) fn static_weekday_opt<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<WeekdayName>, D::Error> {
    #[derive(Deserialize)]
    struct Name(#[serde(deserialize_with = "static_weekday")] WeekdayName);
    Ok(Option::<Name>::deserialize(d)?.map(|n| n.0))
}

/// Monday to Sunday.
pub const WEEKDAYS: [Weekday; 7] = [
    Weekday::Monday,
    Weekday::Tuesday,
    Weekday::Wednesday,
    Weekday::Thursday,
    Weekday::Friday,
    Weekday::Saturday,
    Weekday::Sunday,
];

/// "Mon".
pub fn short_weekday(day: Weekday) -> &'static str {
    &weekday_name(day)[..3]
}

/// "Wed 18 Nov".
pub(super) fn short_date(d: Date) -> String {
    format!(
        "{} {} {}",
        short_weekday(d.weekday()),
        d.day(),
        MONTHS[(d.month() - 1) as usize]
    )
}

/// "Wednesday 2026-11-18".
pub(super) fn long_date(d: Date) -> String {
    format!("{} {d}", weekday_name(d.weekday()))
}

pub(super) const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Whole days from `from` to `to`; negative when `to` is earlier.
pub(super) fn days_between(from: Date, to: Date) -> i32 {
    (to - from).get_days()
}

pub(super) fn relative_days(n: i32) -> String {
    match n {
        0 => "today".into(),
        1 => "tomorrow".into(),
        -1 => "1 day overdue".into(),
        n if n < 0 => format!("{} days overdue", -n),
        n => format!("in {n} days"),
    }
}

pub(super) fn project_suffix(project: &Option<String>) -> String {
    project
        .as_deref()
        .map(|p| format!(" · *{p}*"))
        .unwrap_or_default()
}

pub(super) fn section(md: &mut String, title: &str, count: usize, empty: &str) {
    if count == 0 {
        let _ = write!(md, "\n### {title}\n\n{empty}\n");
    } else {
        let _ = write!(md, "\n### {title} ({count})\n\n");
    }
}

pub(super) fn clean_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A task's notes for the facts: Markdown kept line by line
/// ([`crate::notes::to_markdown`]), cut at [`NOTES_MAX_CHARS`].
pub(super) fn trim_notes(notes: &str) -> String {
    cut_text(&crate::notes::to_markdown(notes), NOTES_MAX_CHARS)
}

/// A log line for the facts: its Markdown kept line by line
/// ([`crate::notes::to_markdown`]), cut at [`LOG_MAX_CHARS`].
pub(super) fn log_text(text: &str) -> String {
    cut_text(&crate::notes::to_markdown(text), LOG_MAX_CHARS)
}

fn cut_text(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

/// Lines after the first indented by `spaces`, so a several-line log
/// line stays inside its Markdown bullet.
pub(super) fn indent_rest(text: &str, spaces: usize) -> String {
    text.replace('\n', &format!("\n{}", " ".repeat(spaces)))
}

pub(super) fn log_bullets(md: &mut String, logs: &[LogEntry]) {
    for l in logs {
        let _ = writeln!(
            md,
            "  - {}: {}",
            short_date(l.date),
            indent_rest(&l.text, 4)
        );
    }
}

pub(super) fn due_suffix(due: Option<Date>) -> String {
    due.map(|d| format!(" · due {}", short_date(d)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::Weekday;

    #[test]
    fn hours_round_to_one_decimal() {
        assert_eq!(hours(0), 0.0);
        assert_eq!(hours(90), 1.5);
        assert_eq!(hours(100), 1.7);
        assert_eq!(hours(360), 6.0);
        assert_eq!(fmt_hours(360), "6h");
        assert_eq!(fmt_hours(0), "0m");
        assert_eq!(fmt_hours(30), "30m");
        assert_eq!(fmt_hours(45), "45m");
        assert_eq!(fmt_hours(60), "1h");
        assert_eq!(fmt_hours(90), "1.5h");
        assert_eq!(fmt_hours(100), "1.7h");
        assert_eq!(fmt_hours(480), "8h");
        assert_eq!(fmt_hours(u32::MAX), "71582788.3h");
    }

    #[test]
    fn weekday_helpers() {
        let names: Vec<&str> = WEEKDAYS.into_iter().map(weekday_name).collect();
        assert_eq!(names[0], "Monday");
        assert_eq!(names[6], "Sunday");
        assert_eq!(short_weekday(Weekday::Thursday), "Thu");
    }
}
