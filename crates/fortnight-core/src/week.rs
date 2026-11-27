//! Week maths and the schedule. Owner: agent B.

use crate::domain::{ReportKey, ReportKind, Schedule, WeekRef};
use jiff::ToSpan;
use jiff::ZonedDateTime;
use jiff::civil::{Date, Time, Weekday};

/// The week containing `date`, for weeks starting on `week_start`.
pub fn week_containing(date: Date, week_start: Weekday) -> WeekRef {
    let back = date.weekday().since(week_start);
    WeekRef::new(date.saturating_sub(i64::from(back).days()))
}

/// The week's number as the app shows it ("W47"): the ISO week number of
/// `start + 3 days`, the Thursday for Monday-start weeks. For other start
/// days this is the ISO week holding most of the shown week's days.
pub fn week_number(week: WeekRef) -> i8 {
    week.start.saturating_add(3.days()).iso_week_date().week()
}

/// The weeks a month calendar shows: from the week containing the 1st to
/// the week containing the last day (four to six weeks).
pub fn month_weeks(date: Date, week_start: Weekday) -> Vec<WeekRef> {
    let first = date.first_of_month();
    let last = date.last_of_month();
    let mut week = week_containing(first, week_start);
    let mut out = Vec::new();
    while week.start <= last {
        out.push(week);
        let next = week.next();
        if next == week {
            break;
        }
        week = next;
    }
    out
}

/// The same day `months` months away, clamped to the month's length
/// (Jan 31 + 1 month = Feb 28).
pub fn add_months(date: Date, months: i32) -> Date {
    date.checked_add(months.months()).unwrap_or(date)
}

/// "November 2026".
pub fn month_label(date: Date) -> String {
    date.strftime("%B %Y").to_string()
}

/// The English three-letter month name: "Jan" to "Dec".
pub fn month_abbr(d: Date) -> &'static str {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS[(d.month() - 1) as usize]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReportJob {
    pub kind: ReportKind,
    pub week: WeekRef,
}

/// The time of day the week's summary falls due, on the last working day.
pub const SUMMARY_DUE_TIME: Time = Time::constant(16, 0, 0, 0);

/// Which reports should be generated now in the background (PLAN.md
/// section 8). Only the Work summary is: recommended tasks are made when the
/// Today tab shows them.
///
/// - The summary for week W is due from 16:00 ([`SUMMARY_DUE_TIME`], local,
///   in `now`'s time zone) on W's last working day until a ready report
///   exists for W. With no working days, the week's last day.
/// - Catch-up: only the most recent week whose due time has passed is
///   considered. Older missed weeks are never returned.
pub fn due_reports(
    now: &ZonedDateTime,
    schedule: &Schedule,
    existing: &[ReportKey],
) -> Vec<ReportJob> {
    let day = last_working_day(schedule);
    let Some(week) = latest_due_week(now, schedule.week_start, day, SUMMARY_DUE_TIME) else {
        return Vec::new();
    };
    let key = ReportKey {
        kind: ReportKind::Summary,
        week_start: week.start,
    };
    if existing.contains(&key) {
        return Vec::new();
    }
    vec![ReportJob {
        kind: ReportKind::Summary,
        week,
    }]
}

/// The working day that comes last in the week (counting from its first
/// day), or the week's last day when none is ticked.
fn last_working_day(schedule: &Schedule) -> Weekday {
    let start = schedule.week_start;
    schedule
        .working_days
        .iter()
        .copied()
        .max_by_key(|day| start.until(*day))
        .unwrap_or(start.previous())
}

/// The most recent week whose due moment (`day` at `time`, local) is at or
/// before `now`. `None` only at the edges of jiff's supported range.
fn latest_due_week(
    now: &ZonedDateTime,
    week_start: Weekday,
    day: Weekday,
    time: Time,
) -> Option<WeekRef> {
    let week = week_containing(now.date(), week_start);
    let due_day = week
        .start
        .checked_add(i64::from(week_start.until(day)).days())
        .ok()?;
    // `to_zoned_datetime` uses the "compatible" strategy: a time in a DST
    // gap moves forward, an ambiguous time takes the earlier instant.
    let due = due_day
        .to_datetime(time)
        .to_zoned_datetime(now.time_zone().clone())
        .ok()?;
    if due.timestamp() <= now.timestamp() {
        Some(week)
    } else {
        // A week earlier, so its due moment has certainly passed.
        Some(week.prev())
    }
}

#[cfg(test)]
mod month_tests {
    use super::*;
    use jiff::civil::date;

    #[test]
    fn month_weeks_cover_the_month() {
        // November 2026: Sun 1st to Mon 30th; Monday weeks Oct 26 .. Nov 30.
        let w = month_weeks(date(2026, 11, 17), Weekday::Monday);
        assert_eq!(w.first().unwrap().start, date(2026, 10, 26));
        assert_eq!(w.last().unwrap().start, date(2026, 11, 30));
        assert_eq!(w.len(), 6);
        // February 2027 starts on a Monday: exactly four weeks.
        assert_eq!(month_weeks(date(2027, 2, 10), Weekday::Monday).len(), 4);
        // Sunday-start weeks.
        let s = month_weeks(date(2026, 11, 1), Weekday::Sunday);
        assert_eq!(s[0].start, date(2026, 11, 1));
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn add_months_clamps() {
        assert_eq!(add_months(date(2026, 1, 31), 1), date(2026, 2, 28));
        assert_eq!(add_months(date(2026, 3, 15), -1), date(2026, 2, 15));
        assert_eq!(month_label(date(2026, 11, 16)), "November 2026");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn zoned(s: &str) -> ZonedDateTime {
        s.parse().unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    /// A UTC instant, viewed in time zone `tz`.
    fn utc_in(ts: &str, tz: &str) -> ZonedDateTime {
        ts.parse::<jiff::Timestamp>().unwrap().in_tz(tz).unwrap()
    }

    fn week(y: i16, m: i8, d: i8) -> WeekRef {
        WeekRef::new(date(y, m, d))
    }

    fn summary(w: WeekRef) -> ReportJob {
        ReportJob {
            kind: ReportKind::Summary,
            week: w,
        }
    }

    /// Monday-start weeks, Monday to Friday.
    fn monday_weeks() -> Schedule {
        Schedule {
            week_start: Weekday::Monday,
            ..Schedule::default()
        }
    }

    #[test]
    fn week_containing_cases() {
        use Weekday::*;
        let cases = [
            // Monday-start weeks.
            (date(2026, 11, 16), Monday, date(2026, 11, 16)),
            (date(2026, 11, 18), Monday, date(2026, 11, 16)),
            (date(2026, 11, 22), Monday, date(2026, 11, 16)),
            (date(2026, 11, 23), Monday, date(2026, 11, 23)),
            (date(2027, 1, 1), Monday, date(2026, 12, 28)),
            // Sunday-start weeks.
            (date(2026, 11, 15), Sunday, date(2026, 11, 15)),
            (date(2026, 11, 16), Sunday, date(2026, 11, 15)),
            (date(2026, 11, 21), Sunday, date(2026, 11, 15)),
            (date(2026, 11, 22), Sunday, date(2026, 11, 22)),
            // Saturday-start, for good measure.
            (date(2026, 11, 20), Saturday, date(2026, 11, 14)),
        ];
        for (d, ws, want) in cases {
            assert_eq!(week_containing(d, ws), WeekRef::new(want), "{d} {ws:?}");
        }
    }

    #[test]
    fn week_numbers() {
        let cases = [
            (week(2026, 11, 16), 47),
            (week(2026, 11, 9), 46),
            (week(2026, 11, 30), 49),
            (week(2026, 12, 28), 53),
            // Sunday-start: number from start + 3 (Wednesday Nov 18).
            (week(2026, 11, 15), 47),
            // Monday 29 Dec 2025 is in ISO week 1 of 2026.
            (week(2025, 12, 29), 1),
        ];
        for (w, want) in cases {
            assert_eq!(week_number(w), want, "{w:?}");
        }
    }

    #[test]
    fn month_abbrs() {
        assert_eq!(month_abbr(Date::constant(2026, 1, 31)), "Jan");
        assert_eq!(month_abbr(Date::constant(2026, 11, 16)), "Nov");
        assert_eq!(month_abbr(Date::constant(2026, 12, 1)), "Dec");
    }

    #[test]
    fn summary_is_due_from_16_00_on_the_last_working_day() {
        let s = monday_weeks();
        let this = week(2026, 11, 16);
        let last = this.prev();
        let cases = [
            ("2026-11-16T08:00:00[UTC]", last),
            // Friday 15:59: still last week's.
            ("2026-11-20T15:59:59[UTC]", last),
            // Friday 16:00: this week's.
            ("2026-11-20T16:00:00[UTC]", this),
            // Sunday night, still this week.
            ("2026-11-22T23:59:00[UTC]", this),
        ];
        for (now, want) in cases {
            assert_eq!(
                due_reports(&zoned(now), &s, &[]),
                vec![summary(want)],
                "now {now}"
            );
        }
    }

    #[test]
    fn the_due_day_follows_the_week_shape() {
        use Weekday::*;
        let due = |week_start, working_days: Vec<Weekday>| {
            last_working_day(&Schedule {
                week_start,
                working_days,
            })
        };
        // The default: Sunday-start weeks, Monday to Friday.
        assert_eq!(last_working_day(&Schedule::default()), Friday);
        assert_eq!(
            due(Monday, vec![Monday, Tuesday, Wednesday, Thursday]),
            Thursday
        );
        // Saturday-start weeks working Saturday to Wednesday: Wednesday,
        // although Saturday is the latest weekday by number.
        assert_eq!(
            due(Saturday, vec![Monday, Tuesday, Wednesday, Saturday, Sunday]),
            Wednesday
        );
        // No working day: the week's last day.
        assert_eq!(due(Sunday, vec![]), Saturday);
        assert_eq!(due(Monday, vec![]), Sunday);
    }

    #[test]
    fn sunday_start_weeks() {
        let s = Schedule::default();
        let this = week(2026, 11, 15);
        assert_eq!(
            due_reports(&zoned("2026-11-20T15:59:00[UTC]"), &s, &[]),
            vec![summary(this.prev())]
        );
        assert_eq!(
            due_reports(&zoned("2026-11-20T16:00:00[UTC]"), &s, &[]),
            vec![summary(this)]
        );
        // Saturday is still the same week.
        assert_eq!(
            due_reports(&zoned("2026-11-21T23:00:00[UTC]"), &s, &[]),
            vec![summary(this)]
        );
    }

    #[test]
    fn an_existing_summary_suppresses_the_job() {
        let s = monday_weeks();
        let this = week(2026, 11, 16);
        let now = zoned("2026-11-20T17:00:00[UTC]");
        let have = ReportKey {
            kind: ReportKind::Summary,
            week_start: this.start,
        };
        assert_eq!(due_reports(&now, &s, &[have]), vec![]);
        // A report for another week, or recommendations, do not count.
        let old = ReportKey {
            kind: ReportKind::Summary,
            week_start: this.prev().start,
        };
        let plan = ReportKey {
            kind: ReportKind::Plan,
            week_start: this.start,
        };
        assert_eq!(due_reports(&now, &s, &[old, plan]), vec![summary(this)]);
    }

    #[test]
    fn catch_up_returns_only_the_most_recent_week() {
        // App closed for three weeks; nothing exists. Only last week's
        // summary is returned; older missing ones are not.
        let s = monday_weeks();
        let now = zoned("2026-11-18T12:00:00[UTC]"); // Wednesday
        assert_eq!(due_reports(&now, &s, &[]), vec![summary(week(2026, 11, 9))]);
        let existing = [ReportKey {
            kind: ReportKind::Summary,
            week_start: date(2026, 11, 9),
        }];
        assert_eq!(due_reports(&now, &s, &existing), vec![]);
    }

    #[test]
    fn due_moment_is_local_time_across_dst() {
        let s = monday_weeks();
        let ny = "America/New_York";
        // DST began Sunday 2026-03-08. Friday 16:00 EDT is 20:00 UTC.
        let this = week(2026, 3, 9);
        let before = utc_in("2026-03-13T19:59:00Z", ny);
        let after = utc_in("2026-03-13T20:00:00Z", ny);
        assert_eq!(due_reports(&before, &s, &[]), vec![summary(this.prev())]);
        assert_eq!(due_reports(&after, &s, &[]), vec![summary(this)]);

        // The week before DST: Friday 16:00 EST is 21:00 UTC.
        let prev = week(2026, 3, 2);
        let before = utc_in("2026-03-06T20:59:00Z", ny);
        let after = utc_in("2026-03-06T21:00:00Z", ny);
        assert_eq!(due_reports(&before, &s, &[]), vec![summary(prev.prev())]);
        assert_eq!(due_reports(&after, &s, &[]), vec![summary(prev)]);
    }

    #[test]
    fn local_date_decides_the_week() {
        // Sunday 23:00 in New York is Monday 04:00 UTC, but the local date
        // (Sunday) decides the week: still the one starting Nov 16.
        let s = monday_weeks();
        let now = zoned("2026-11-22T23:00:00-05:00[America/New_York]");
        assert_eq!(
            due_reports(&now, &s, &[]),
            vec![summary(week(2026, 11, 16))]
        );
    }
}
