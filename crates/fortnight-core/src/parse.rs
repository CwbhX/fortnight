//! Quick-add parser. Owner: agent B.
//!
//! `Write Q4 report fri #work ~2h !` → title "Write Q4 report", due the
//! coming Friday, project "work", estimate 120 minutes, priority 1.
//! Tokens are separated by whitespace and may appear in any order:
//!
//! - **Dates**: `today`, `tomorrow` (`tmr`), weekday names and 3-letter
//!   abbreviations (`fri` = the next Friday, never today), `nov 20` /
//!   `20 nov` (this year, or next year if already past), ISO `2026-11-20`,
//!   and `+3d` / `+2w`. The last date token wins.
//! - **Project**: `#work`. The last one wins.
//! - **Estimate**: `~` then `2h`, `30m`, `1h30m` or `1.5h`
//!   (case-insensitive). Zero, unparseable or absurd values (over 7 days)
//!   are not estimates and stay in the title. The last estimate wins.
//! - **Priority**: a standalone `!`, `!!` or `!!!` (1–3). The highest wins.
//!   A `!` attached to a word ("urgent!") is part of the title.
//!
//! Everything else, in order, is the title.
//!
//! [`parse_estimate`] and [`format_estimate`] read and write estimates on
//! their own (the `~` is optional there) for the task edit form.
//!
//! Known limitation: a title word that is also a date token ("Call Monday
//! Corp", "Buy 2 may ...") is read as a date. Prototype scope accepts this.

use jiff::Span;
use jiff::civil::{Date, Weekday};

use crate::domain::MAX_PRIORITY;

/// The largest estimate accepted: seven days, in minutes.
const MAX_ESTIMATE_MINUTES: u32 = 7 * 24 * 60;

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
        } else if let Some(m) = tok.strip_prefix('~').and_then(estimate_body) {
            out.estimate_minutes = Some(m);
        } else if let Some(p) = priority(tok) {
            out.priority = out.priority.max(p);
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

/// Parses an estimate such as `~2h`, `30m`, `1h30m` or `1.5h`. The leading
/// `~` is optional and surrounding whitespace is ignored. `None` for empty,
/// zero, unparseable or over seven days. Used by the task edit form.
pub fn parse_estimate(s: &str) -> Option<u32> {
    let s = s.trim();
    estimate_body(s.strip_prefix('~').unwrap_or(s))
}

/// Formats minutes the way [`parse_estimate`] reads them: `30m`, `2h`,
/// `1h30m`. Zero formats as `0m` (which does not parse back).
pub fn format_estimate(minutes: u32) -> String {
    let (h, m) = (minutes / 60, minutes % 60);
    match (h, m) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m}m"),
    }
}

/// The part of an estimate after the optional `~`: `2h`, `30m`, `1h30m`,
/// `1.5h`, any case. Hours may have a decimal part (rounded to the nearest
/// minute); minutes and the `XhYm` form are whole numbers.
fn estimate_body(body: &str) -> Option<u32> {
    let t = body.to_ascii_lowercase();
    let minutes: u64 = if let Some((h, m)) = t.split_once('h').filter(|(_, m)| !m.is_empty()) {
        // `1h30m`
        let m = m.strip_suffix('m')?;
        whole(h)?.checked_mul(60)?.checked_add(whole(m)?)?
    } else if let Some(h) = t.strip_suffix('h') {
        hours(h)?
    } else {
        whole(t.strip_suffix('m')?)?
    };
    if minutes == 0 || minutes > u64::from(MAX_ESTIMATE_MINUTES) {
        return None;
    }
    u32::try_from(minutes).ok()
}

/// A non-empty run of ASCII digits.
fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// A whole number of ASCII digits, `None` if it overflows.
fn whole(s: &str) -> Option<u64> {
    is_digits(s).then(|| s.parse().ok()).flatten()
}

/// `2` or `1.5` hours, in minutes rounded to the nearest minute.
fn hours(s: &str) -> Option<u64> {
    let Some((int, frac)) = s.split_once('.') else {
        return whole(s)?.checked_mul(60);
    };
    // Validate both halves, then let f64 do the arithmetic. Anything large
    // enough to lose precision is far over the limit anyway.
    if !is_digits(int) || !is_digits(frac) {
        return None;
    }
    let minutes = (s.parse::<f64>().ok()? * 60.0).round();
    if minutes > f64::from(MAX_ESTIMATE_MINUTES) {
        return None;
    }
    Some(minutes as u64)
}

/// A standalone `!`, `!!` or `!!!`.
fn priority(token: &str) -> Option<u8> {
    let n = token.len();
    (n >= 1 && n <= usize::from(MAX_PRIORITY) && token.bytes().all(|b| b == b'!'))
        .then_some(n as u8)
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

    /// A full expected result, estimate and priority included.
    fn qx(
        title: &str,
        project: Option<&str>,
        due: Option<Date>,
        estimate: Option<u32>,
        priority: u8,
    ) -> QuickAdd {
        QuickAdd {
            estimate_minutes: estimate,
            priority,
            ..qa(title, project, due)
        }
    }

    #[test]
    fn estimate_forms() {
        let cases: &[(&str, Option<u32>)] = &[
            ("~2h", Some(120)),
            ("~30m", Some(30)),
            ("~1h30m", Some(90)),
            ("~1.5h", Some(90)),
            ("~90m", Some(90)),
            ("~2H", Some(120)),
            ("~1H30M", Some(90)),
            ("~0.25h", Some(15)),
            ("~0.5h", Some(30)),
            ("~1.33h", Some(80)),
            ("~1.5000000000000000000000001h", Some(90)),
            ("~1h0m", Some(60)),
            ("~1h90m", Some(150)),
            ("~0h5m", Some(5)),
            ("~1m", Some(1)),
            ("~168h", Some(10080)),
            ("~10080m", Some(10080)),
            ("~7.0h", Some(420)),
            // Without the `~`, and with surrounding whitespace.
            ("2h", Some(120)),
            ("  45m  ", Some(45)),
            (" ~1h30m\t", Some(90)),
            // Not estimates.
            ("", None),
            ("   ", None),
            ("~", None),
            ("~0h", None),
            ("~0m", None),
            ("~0h0m", None),
            ("~0.001h", None),
            ("~-2h", None),
            ("~2", None),
            ("~h", None),
            ("~m", None),
            ("~2d", None),
            ("~2x", None),
            ("~1.5m", None),
            ("~1.h", None),
            ("~.5h", None),
            ("~1.2.3h", None),
            ("~1.5h30m", None),
            ("~1h30", None),
            ("~1h30h", None),
            ("~h30m", None),
            ("~2hm", None),
            ("~~2h", None),
            ("~ 2h", None),
            ("~1h 30m", None),
            ("~+2h", None),
            ("~169h", None),
            ("~10081m", None),
            ("~167h61m", None),
            ("~168.1h", None),
            ("~99999999999999999999h", None),
            ("~99999999999999999999m", None),
            ("~1e3h", None),
            ("~nanh", None),
            ("~infh", None),
        ];
        for &(input, want) in cases {
            assert_eq!(parse_estimate(input), want, "input {input:?}");
        }
    }

    #[test]
    fn format_estimate_examples() {
        assert_eq!(format_estimate(30), "30m");
        assert_eq!(format_estimate(59), "59m");
        assert_eq!(format_estimate(60), "1h");
        assert_eq!(format_estimate(120), "2h");
        assert_eq!(format_estimate(90), "1h30m");
        assert_eq!(format_estimate(61), "1h1m");
        assert_eq!(format_estimate(10080), "168h");
        assert_eq!(format_estimate(0), "0m");
    }

    #[test]
    fn estimate_round_trip() {
        for m in 1..=MAX_ESTIMATE_MINUTES {
            let text = format_estimate(m);
            assert_eq!(parse_estimate(&text), Some(m), "minutes {m} as {text:?}");
            assert_eq!(
                parse_estimate(&format!("~{text}")),
                Some(m),
                "minutes {m} as ~{text}"
            );
        }
    }

    #[test]
    fn quick_add_estimates_and_priority() {
        let fri = Some(date(2026, 11, 20));
        let tmr = Some(date(2026, 11, 17));
        let cases: Vec<(&str, QuickAdd)> = vec![
            // The example from PLAN.md 4.2.
            (
                "Write Q4 report fri #work ~2h !",
                qx("Write Q4 report", Some("work"), fri, Some(120), 1),
            ),
            // Any order.
            (
                "! ~2h #work fri Write Q4 report",
                qx("Write Q4 report", Some("work"), fri, Some(120), 1),
            ),
            (
                "Write ~2h Q4 ! report #work fri",
                qx("Write Q4 report", Some("work"), fri, Some(120), 1),
            ),
            (
                "~30m Pay rent nov 20 !! #home",
                qx("Pay rent", Some("home"), fri, Some(30), 2),
            ),
            (
                "Pay rent ~1h30m 20 nov !!!",
                qx("Pay rent", None, fri, Some(90), 3),
            ),
            (
                "Review PR ~1.5h tomorrow",
                qx("Review PR", None, tmr, Some(90), 0),
            ),
            ("Review PR ~90m", qx("Review PR", None, None, Some(90), 0)),
            ("Review PR ~2H", qx("Review PR", None, None, Some(120), 0)),
            ("Fix bug !!", qx("Fix bug", None, None, None, 2)),
            // Last estimate wins.
            ("Plan ~2h ~30m", qx("Plan", None, None, Some(30), 0)),
            // An invalid estimate does not override a valid one.
            ("Plan ~30m ~0h", qx("Plan ~0h", None, None, Some(30), 0)),
            // Highest priority wins, whatever the order.
            ("Plan !!! !", qx("Plan", None, None, None, 3)),
            ("Plan ! !!", qx("Plan", None, None, None, 2)),
            // Only tokens: empty title.
            ("~2h !", qx("", None, None, Some(120), 1)),
            // Stay in the title.
            ("Fix it urgent!", qx("Fix it urgent!", None, None, None, 0)),
            (
                "!important thing",
                qx("!important thing", None, None, None, 0),
            ),
            ("Wow !!!!", qx("Wow !!!!", None, None, None, 0)),
            ("Hey !?", qx("Hey !?", None, None, None, 0)),
            (
                "About ~ an hour",
                qx("About ~ an hour", None, None, None, 0),
            ),
            ("Nap ~0m", qx("Nap ~0m", None, None, None, 0)),
            ("Nap ~0h", qx("Nap ~0h", None, None, None, 0)),
            ("Debt ~-2h", qx("Debt ~-2h", None, None, None, 0)),
            ("Trip ~3d", qx("Trip ~3d", None, None, None, 0)),
            ("Trip ~200h", qx("Trip ~200h", None, None, None, 0)),
            // A space splits `~1h 30m`: only `~1h` is an estimate.
            ("Trip ~1h 30m", qx("Trip 30m", None, None, Some(60), 0)),
            ("Call ~bob", qx("Call ~bob", None, None, None, 0)),
            // A bare estimate without `~` is a title word in quick-add.
            ("Run 5m sprints", qx("Run 5m sprints", None, None, None, 0)),
            ("Buy 2h glue", qx("Buy 2h glue", None, None, None, 0)),
        ];
        for (input, want) in cases {
            assert_eq!(parse_quick_add(input, TODAY), want, "input {input:?}");
        }
    }
}
