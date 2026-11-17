//! Week maths and the schedule. Owner: agent B.

use crate::domain::{ReportKey, ReportKind, Schedule, WeekRef};
use jiff::ZonedDateTime;
use jiff::civil::{Date, Weekday};

/// The week containing `date`, for weeks starting on `week_start`.
pub fn week_containing(_date: Date, _week_start: Weekday) -> WeekRef {
    todo!("agent B")
}

/// A short label such as "Week 46 · Nov 9–15". The number is the ISO week
/// number of `start + 3 days` (the Thursday for Monday-start weeks).
pub fn week_label(_week: WeekRef) -> String {
    todo!("agent B")
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
    _now: &ZonedDateTime,
    _schedule: &Schedule,
    _existing: &[ReportKey],
) -> Vec<ReportJob> {
    todo!("agent B")
}
