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

/// A short label such as "Week 46 · Nov 9–15". The number is the ISO week
/// number of `start + 3 days` (the Thursday for Monday-start weeks).
pub fn week_label(week: WeekRef) -> String {
    let start = week.start;
    let end = week.end();
    let number = start.saturating_add(3.days()).iso_week_date().week();
    let range = if start.month() == end.month() {
        format!("{} {}–{}", month_abbr(start), start.day(), end.day())
    } else {
        format!(
            "{} {}–{} {}",
            month_abbr(start),
            start.day(),
            month_abbr(end),
            end.day()
        )
    };
    format!("Week {number} · {range}")
}

/// The first day of `date`'s month.
pub fn month_start(date: Date) -> Date {
    date.first_of_month()
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
        week = week.next();
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

fn month_abbr(d: Date) -> &'static str {
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

/// Which reports should be generated now (PLAN.md section 8).
///
/// - The summary for week W is due from W's `summary_day` at `summary_time`
///   (local, in `now`'s time zone) until a ready report exists for W.
/// - The plan for week W is due from W's `plan_day` at `plan_time` until one
///   exists.
/// - Catch-up: only the most recent week whose due time has passed is
///   considered, per kind. Older missed weeks are never returned.
pub fn due_reports(
    now: &ZonedDateTime,
    schedule: &Schedule,
    existing: &[ReportKey],
) -> Vec<ReportJob> {
    let kinds = [
        (
            ReportKind::Summary,
            schedule.summary_day,
            schedule.summary_time,
        ),
        (ReportKind::Plan, schedule.plan_day, schedule.plan_time),
    ];
    kinds
        .into_iter()
        .filter_map(|(kind, day, time)| {
            let week = latest_due_week(now, schedule.week_start, day, time)?;
            let key = ReportKey {
                kind,
                week_start: week.start,
            };
            (!existing.contains(&key)).then_some(ReportJob { kind, week })
        })
        .collect()
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
        assert_eq!(month_start(date(2026, 11, 16)), date(2026, 11, 1));
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

    fn plan(w: WeekRef) -> ReportJob {
        ReportJob {
            kind: ReportKind::Plan,
            week: w,
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
    fn week_labels() {
        let cases = [
            (week(2026, 11, 16), "Week 47 · Nov 16–22"),
            (week(2026, 11, 9), "Week 46 · Nov 9–15"),
            (week(2026, 11, 30), "Week 49 · Nov 30–Dec 6"),
            (week(2026, 12, 28), "Week 53 · Dec 28–Jan 3"),
            // Sunday-start: number from start + 3 (Wednesday Nov 18).
            (week(2026, 11, 15), "Week 47 · Nov 15–21"),
        ];
        for (w, want) in cases {
            assert_eq!(week_label(w), want);
        }
    }

    #[test]
    fn due_reports_default_schedule() {
        let s = Schedule::default();
        let this = week(2026, 11, 16);
        let last = this.prev();
        let cases: Vec<(&str, Vec<ReportJob>)> = vec![
            // Monday 07:59: neither this week's plan nor summary yet.
            ("2026-11-16T07:59:00[UTC]", vec![summary(last), plan(last)]),
            // Monday 08:00: this week's plan is due.
            ("2026-11-16T08:00:00[UTC]", vec![summary(last), plan(this)]),
            // Friday 15:59: summary still last week's.
            ("2026-11-20T15:59:59[UTC]", vec![summary(last), plan(this)]),
            // Friday 16:00: this week's summary.
            ("2026-11-20T16:00:00[UTC]", vec![summary(this), plan(this)]),
            // Sunday night, still this week.
            ("2026-11-22T23:59:00[UTC]", vec![summary(this), plan(this)]),
        ];
        for (now, want) in cases {
            assert_eq!(due_reports(&zoned(now), &s, &[]), want, "now {now}");
        }
    }

    #[test]
    fn existing_reports_suppress_jobs() {
        let s = Schedule::default();
        let this = week(2026, 11, 16);
        let now = zoned("2026-11-20T17:00:00[UTC]");
        let have_summary = ReportKey {
            kind: ReportKind::Summary,
            week_start: this.start,
        };
        assert_eq!(due_reports(&now, &s, &[have_summary]), vec![plan(this)]);

        let have_plan = ReportKey {
            kind: ReportKind::Plan,
            week_start: this.start,
        };
        assert_eq!(due_reports(&now, &s, &[have_summary, have_plan]), vec![]);

        // A report for another week does not count.
        let old = ReportKey {
            kind: ReportKind::Summary,
            week_start: this.prev().start,
        };
        assert_eq!(
            due_reports(&now, &s, &[old]),
            vec![summary(this), plan(this)]
        );
    }

    #[test]
    fn catch_up_returns_only_most_recent_week() {
        // App closed for three weeks; nothing exists. Only the latest weeks
        // whose moments have passed are returned, once per kind.
        let s = Schedule::default();
        let now = zoned("2026-11-18T12:00:00[UTC]"); // Wednesday
        assert_eq!(
            due_reports(&now, &s, &[]),
            vec![summary(week(2026, 11, 9)), plan(week(2026, 11, 16))]
        );
        // Last week's summary exists; older ones are missing but not returned.
        let existing = [ReportKey {
            kind: ReportKind::Summary,
            week_start: date(2026, 11, 9),
        }];
        assert_eq!(
            due_reports(&now, &s, &existing),
            vec![plan(week(2026, 11, 16))]
        );
    }

    #[test]
    fn due_moment_is_local_time_across_dst() {
        let s = Schedule::default();
        let ny = "America/New_York";
        // DST began Sunday 2026-03-08. Friday 16:00 EDT is 20:00 UTC.
        let this = week(2026, 3, 9);
        let before = utc_in("2026-03-13T19:59:00Z", ny);
        let after = utc_in("2026-03-13T20:00:00Z", ny);
        assert_eq!(due_reports(&before, &s, &[])[0], summary(this.prev()));
        assert_eq!(due_reports(&after, &s, &[])[0], summary(this));

        // The week before DST: Friday 16:00 EST is 21:00 UTC.
        let prev = week(2026, 3, 2);
        let before = utc_in("2026-03-06T20:59:00Z", ny);
        let after = utc_in("2026-03-06T21:00:00Z", ny);
        assert_eq!(due_reports(&before, &s, &[])[0], summary(prev.prev()));
        assert_eq!(due_reports(&after, &s, &[])[0], summary(prev));

        // DST ended Sunday 2026-11-01: Monday 08:00 EST is 13:00 UTC.
        let nov2 = week(2026, 11, 2);
        let before = utc_in("2026-11-02T12:59:00Z", ny);
        let after = utc_in("2026-11-02T13:00:00Z", ny);
        assert_eq!(due_reports(&before, &s, &[])[1], plan(nov2.prev()));
        assert_eq!(due_reports(&after, &s, &[])[1], plan(nov2));
    }

    #[test]
    fn nonexistent_local_time_resolves_forward() {
        // Plan due Sunday 02:30, which does not exist on 2026-03-08 in New
        // York (clocks jump 02:00 → 03:00). It resolves to 03:30 EDT.
        let s = Schedule {
            plan_day: Weekday::Sunday,
            plan_time: Time::constant(2, 30, 0, 0),
            ..Schedule::default()
        };
        let w = week(2026, 3, 2); // Monday-start week holding Sunday Mar 8.
        let ny = "America/New_York";
        let before = zoned(&format!("2026-03-08T03:29:00-04:00[{ny}]"));
        let after = zoned(&format!("2026-03-08T03:30:00-04:00[{ny}]"));
        assert_eq!(due_reports(&before, &s, &[])[1], plan(w.prev()));
        assert_eq!(due_reports(&after, &s, &[])[1], plan(w));
    }

    #[test]
    fn local_date_decides_the_week() {
        // Sunday 23:00 in New York is Monday 04:00 UTC, but the local date
        // (Sunday) decides the week: still the one starting Nov 16.
        let s = Schedule::default();
        let now = zoned("2026-11-22T23:00:00-05:00[America/New_York]");
        assert_eq!(
            due_reports(&now, &s, &[]),
            vec![summary(week(2026, 11, 16)), plan(week(2026, 11, 16))]
        );
    }
}
