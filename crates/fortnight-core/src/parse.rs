//! Quick-add parser. Owner: agent B.
//!
//! `Write Q4 report fri #work` → title "Write Q4 report", due the coming
//! Friday, project "work". Date tokens: `today`, `tomorrow` (`tmr`),
//! weekday names and 3-letter abbreviations (`fri` = the next Friday,
//! never today), `nov 20` / `20 nov` (this year, or next year if already
//! past), ISO `2026-11-20`, and `+3d` / `+2w`. `#project` sets the project.
//! The last date token and the last `#project` win; everything else,
//! in order, is the title.
//!
//! Known limitation: a title word that is also a date token ("Call Monday
//! Corp", "Buy 2 may ...") is read as a date. Prototype scope accepts this.

use jiff::Span;
use jiff::civil::{Date, Weekday};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuickAdd {
    pub title: String,
    pub project: Option<String>,
    pub due_on: Option<Date>,
    /// `~2h`, `~30m`, `~1h30m`, `~1.5h` (round 2, agent B).
    pub estimate_minutes: Option<u32>,
    /// `!` = 1, `!!` = 2, `!!!` = 3 (round 2, agent B).
    pub priority: u8,
}

/// Parses a quick-add line relative to `today`.
pub fn parse_quick_add(input: &str, today: Date) -> QuickAdd {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    let mut out = QuickAdd::default();
    let mut title: Vec<&str> = Vec::new();

    let mut i = 0;
    while i < tokens.len() {
        if let Some(next) = tokens.get(i + 1)
            && let Some(d) = month_day(tokens[i], next, today)
        {
            out.due_on = Some(d);
            i += 2;
            continue;
        }
        let tok = tokens[i];
        if let Some(d) = single_date(tok, today) {
            out.due_on = Some(d);
        } else if let Some(p) = tok.strip_prefix('#').filter(|p| !p.is_empty()) {
            out.project = Some(p.to_string());
        } else {
            title.push(tok);
        }
        i += 1;
    }

    out.title = title.join(" ");
    out
}

/// Parses a whole string as one date (`fri`, `tomorrow`, `nov 20`, `+3d`,
/// `2026-11-20`, ...). Used by the task edit form. `None` if not a date.
pub fn parse_date(input: &str, today: Date) -> Option<Date> {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    match tokens.as_slice() {
        [one] => single_date(one, today),
        [a, b] => month_day(a, b, today),
        _ => None,
    }
}

/// A one-token date: keyword, weekday, ISO date or relative offset.
fn single_date(token: &str, today: Date) -> Option<Date> {
    let t = token.to_ascii_lowercase();
    match t.as_str() {
        "today" => return Some(today),
        "tomorrow" | "tmr" => return today.tomorrow().ok(),
        _ => {}
    }
    if let Some(wd) = weekday(&t) {
        // `nth_weekday(1, ..)` never counts `today` itself.
        return today.nth_weekday(1, wd).ok();
    }
    if let Some(rest) = t.strip_prefix('+') {
        return relative(rest, today);
    }
    iso_date(&t)
}

/// `3d` or `2w` (the part after `+`).
fn relative(rest: &str, today: Date) -> Option<Date> {
    let unit = rest.chars().last()?;
    let digits = &rest[..rest.len() - unit.len_utf8()];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: i64 = digits.parse().ok()?;
    let span = match unit {
        'd' => Span::new().try_days(n).ok()?,
        'w' => Span::new().try_weeks(n).ok()?,
        _ => return None,
    };
    today.checked_add(span).ok()
}

/// Strict `YYYY-MM-DD`.
fn iso_date(t: &str) -> Option<Date> {
    let parts: Vec<&str> = t.split('-').collect();
    let [y, m, d] = parts.as_slice() else {
        return None;
    };
    let all_digits = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(y, 4) || !all_digits(m, 2) || !all_digits(d, 2) {
        return None;
    }
    Date::new(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?).ok()
}

/// `nov 20`, `20 nov`, `november 20`. This year, or next year if the date
/// has already passed. Invalid combinations (`feb 30`) are `None`.
fn month_day(a: &str, b: &str, today: Date) -> Option<Date> {
    let (month, day) = match (month(a), day_number(b)) {
        (Some(m), Some(d)) => (m, d),
        _ => (month(b)?, day_number(a)?),
    };
    let this_year = Date::new(today.year(), month, day).ok();
    match this_year {
        Some(d) if d >= today => Some(d),
        // Past, or (for Feb 29) not valid this year: try next year.
        _ => Date::new(today.year().checked_add(1)?, month, day).ok(),
    }
}

fn day_number(s: &str) -> Option<i8> {
    if s.is_empty() || s.len() > 2 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn month(s: &str) -> Option<i8> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let s = s.to_ascii_lowercase();
    if s == "sept" {
        return Some(9);
    }
    MONTHS
        .iter()
        .position(|name| s == *name || (s.len() == 3 && name.starts_with(s.as_str())))
        .map(|i| i as i8 + 1)
}

/// Full name or 3-letter abbreviation, already lowercased.
fn weekday(s: &str) -> Option<Weekday> {
    const DAYS: [(&str, Weekday); 7] = [
        ("monday", Weekday::Monday),
        ("tuesday", Weekday::Tuesday),
        ("wednesday", Weekday::Wednesday),
        ("thursday", Weekday::Thursday),
        ("friday", Weekday::Friday),
        ("saturday", Weekday::Saturday),
        ("sunday", Weekday::Sunday),
    ];
    DAYS.iter()
        .find(|(name, _)| s == *name || (s.len() == 3 && name.starts_with(s)))
        .map(|&(_, wd)| wd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    /// Monday 16 November 2026.
    const TODAY: Date = Date::constant(2026, 11, 16);

    fn qa(title: &str, project: Option<&str>, due: Option<Date>) -> QuickAdd {
        QuickAdd {
            title: title.to_string(),
            project: project.map(str::to_string),
            due_on: due,
            ..QuickAdd::default()
        }
    }

    #[test]
    fn single_date_tokens() {
        let cases: &[(&str, Option<Date>)] = &[
            ("today", Some(date(2026, 11, 16))),
            ("TODAY", Some(date(2026, 11, 16))),
            ("tomorrow", Some(date(2026, 11, 17))),
            ("tmr", Some(date(2026, 11, 17))),
            ("tue", Some(date(2026, 11, 17))),
            ("Wednesday", Some(date(2026, 11, 18))),
            ("fri", Some(date(2026, 11, 20))),
            ("sun", Some(date(2026, 11, 22))),
            // Same weekday as today: next week, never today.
            ("mon", Some(date(2026, 11, 23))),
            ("monday", Some(date(2026, 11, 23))),
            ("2026-11-20", Some(date(2026, 11, 20))),
            ("2027-02-28", Some(date(2027, 2, 28))),
            ("+3d", Some(date(2026, 11, 19))),
            ("+0d", Some(date(2026, 11, 16))),
            ("+2w", Some(date(2026, 11, 30))),
            ("+1W", Some(date(2026, 11, 23))),
            ("nov 20", Some(date(2026, 11, 20))),
            ("20 nov", Some(date(2026, 11, 20))),
            ("november 20", Some(date(2026, 11, 20))),
            ("Nov 16", Some(date(2026, 11, 16))),
            ("sept 3", Some(date(2027, 9, 3))),
            // Already past this year: next year.
            ("nov 15", Some(date(2027, 11, 15))),
            ("jan 5", Some(date(2027, 1, 5))),
            ("dec 31", Some(date(2026, 12, 31))),
            // Not dates.
            ("", None),
            ("   ", None),
            ("feb 30", None),
            ("2026-02-30", None),
            ("2026-1-5", None),
            ("+d", None),
            ("+3x", None),
            ("+-3d", None),
            ("nov", None),
            ("20", None),
            ("fr", None),
            ("nov 20 extra", None),
            ("hello", None),
        ];
        for &(input, want) in cases {
            assert_eq!(parse_date(input, TODAY), want, "input {input:?}");
        }
    }

    #[test]
    fn quick_add_lines() {
        let cases: Vec<(&str, QuickAdd)> = vec![
            (
                "Write Q4 report fri #work",
                qa("Write Q4 report", Some("work"), Some(date(2026, 11, 20))),
            ),
            (
                "#work tomorrow Write   Q4 report",
                qa("Write Q4 report", Some("work"), Some(date(2026, 11, 17))),
            ),
            (
                "Pay rent nov 20",
                qa("Pay rent", None, Some(date(2026, 11, 20))),
            ),
            (
                "Pay rent 20 nov #home",
                qa("Pay rent", Some("home"), Some(date(2026, 11, 20))),
            ),
            (
                "Renew passport jan 5",
                qa("Renew passport", None, Some(date(2027, 1, 5))),
            ),
            ("Ship it +2w", qa("Ship it", None, Some(date(2026, 11, 30)))),
            (
                "Ship it 2026-12-01",
                qa("Ship it", None, Some(date(2026, 12, 1))),
            ),
            // Last date and last project win.
            (
                "Plan #a fri #b tomorrow",
                qa("Plan", Some("b"), Some(date(2026, 11, 17))),
            ),
            // Only a date: empty title.
            ("fri", qa("", None, Some(date(2026, 11, 20)))),
            ("#work", qa("", Some("work"), None)),
            // Empty input.
            ("", qa("", None, None)),
            ("  \t ", qa("", None, None)),
            // Invalid date stays in the title.
            ("Party feb 30", qa("Party feb 30", None, None)),
            ("Bare # sign", qa("Bare # sign", None, None)),
            ("Plain title", qa("Plain title", None, None)),
            // Known limitation: weekday words are always dates.
            (
                "Call Monday Corp",
                qa("Call Corp", None, Some(date(2026, 11, 23))),
            ),
        ];
        for (input, want) in cases {
            assert_eq!(parse_quick_add(input, TODAY), want, "input {input:?}");
        }
    }

    #[test]
    fn month_day_on_leap_day() {
        // Feb 29 is invalid in 2026 and 2027: not a date.
        assert_eq!(parse_date("feb 29", TODAY), None);
        // From 2027-11-16, Feb 29 next year (2028) is valid.
        assert_eq!(
            parse_date("feb 29", date(2027, 11, 16)),
            Some(date(2028, 2, 29))
        );
    }
}
