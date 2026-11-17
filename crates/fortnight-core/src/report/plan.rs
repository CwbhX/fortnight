//! Plan facts (today and the week ahead), their Markdown, the
//! recommended-tasks prompt, and the day and column loads behind them and
//! the Week tab's load bars.

use super::format::*;
use super::history::*;
use super::{DATA_RULE, Prepared, facts_json};
use crate::domain::{Event, Priority, ReportKind, Settings, Task, TaskStatus, WeekRef};
use crate::llm::ChatRequest;
use jiff::ToSpan;
use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// How many undated open tasks the plan facts list.
pub(super) const MAX_UNDATED: usize = 10;

/// How far ahead of today the plan looks for upcoming deadlines.
pub(super) const LOOKAHEAD_DAYS: i64 = 14;

/// Facts for advice on `week` (the week ahead), as seen on `today`.
/// Private tasks are left out of every list, total and load.
///
/// They also deserialise, so a client that reads them back from the
/// request (`decision.rs`) uses these field names rather than its own copy.
/// Weekday names deserialise to the same static names; anything else is
/// an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanFacts {
    pub today: Date,
    #[serde(deserialize_with = "static_weekday")]
    pub today_weekday: WeekdayName,
    pub week_start: Date,
    pub week_end: Date,
    /// Working days from today (inclusive) to the week's end, each with its
    /// capacity and the estimated load already planned on it.
    pub remaining_working_days: Vec<PlanDay>,
    /// Open tasks due before today.
    pub overdue: Vec<PlanTask>,
    /// Open tasks due from today to the week's end.
    pub due_this_week: Vec<PlanTask>,
    /// Open tasks due after the week but within 14 days of today, or due
    /// later but planned for this week (or carried over to today).
    pub due_soon: Vec<PlanTask>,
    /// Open tasks without a due date: every one on the Today list, then up
    /// to 10 more, those planned for this week first, then list order.
    pub undated: Vec<PlanTask>,
    pub totals: PlanTotals,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanDay {
    pub date: Date,
    #[serde(deserialize_with = "static_weekday")]
    pub weekday: WeekdayName,
    /// Focus time available (`Settings::focus_minutes_per_day`).
    pub capacity_minutes: u32,
    pub capacity_hours: f64,
    /// Sum of the estimates of open tasks whose day (`Task::day`, the
    /// planned day) is this date.
    pub load_minutes: u32,
    pub load_hours: f64,
    /// Open tasks on this day with no estimate (not in `load_minutes`).
    pub unestimated: usize,
    /// `load_minutes` is above `capacity_minutes`.
    pub over_capacity: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanTask {
    pub id: i64,
    pub title: String,
    pub project: Option<String>,
    pub due_on: Option<Date>,
    #[serde(default, deserialize_with = "static_weekday_opt")]
    pub due_weekday: Option<WeekdayName>,
    /// Negative when overdue; 0 = due today.
    pub days_until_due: Option<i32>,
    pub overdue: bool,
    /// The day the user means to do it, if set.
    pub planned_for: Option<Date>,
    /// On the Today list: planned for today, or for an earlier day and
    /// carried over because it is still open.
    pub on_today_list: bool,
    #[serde(default, deserialize_with = "static_weekday_opt")]
    pub planned_weekday: Option<WeekdayName>,
    /// `null` when there is no estimate.
    pub estimate_minutes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimate_hours: Option<f64>,
    /// 0 low, 1 medium, 2 or more high.
    pub priority: Priority,
    /// How many times the deadline has moved later, all time.
    pub deadline_moves: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanTotals {
    pub open: usize,
    pub overdue: usize,
    pub due_this_week: usize,
    pub due_soon: usize,
    /// All undated open tasks, including any not listed.
    pub undated: usize,
    /// Estimated minutes of open tasks due by the week's end, overdue ones
    /// included. Tasks without an estimate add nothing.
    pub due_estimate_minutes: u32,
    pub due_estimate_hours: f64,
    /// Capacity over the remaining working days.
    pub available_minutes: u32,
    pub available_hours: f64,
    /// Overdue or due-this-week tasks with no estimate.
    pub due_without_estimate: usize,
    /// All open tasks with no estimate.
    pub without_estimate: usize,
    /// `due_estimate_minutes` is above `available_minutes`.
    pub overloaded: bool,
}

/// Builds plan facts from open tasks, relative to `today`. `events` is the
/// whole history; it is only used to count deadline moves (all time).
pub fn plan_facts(
    tasks: &[Task],
    events: &[Event],
    week: WeekRef,
    today: Date,
    settings: &Settings,
) -> PlanFacts {
    let week_end = week.end();
    let horizon = today.saturating_add(LOOKAHEAD_DAYS.days());
    let working = &settings.schedule.working_days;
    let tasks = shareable(tasks);
    let history = History::new(&tasks, events);

    let remaining_working_days: Vec<PlanDay> = loads(tasks.iter().copied(), week, settings)
        .into_iter()
        .filter(|l| l.date >= today && working.contains(&l.date.weekday()))
        .map(|l| PlanDay {
            date: l.date,
            weekday: weekday_name(l.date.weekday()),
            capacity_minutes: l.capacity_minutes,
            capacity_hours: hours(l.capacity_minutes),
            load_minutes: l.load_minutes,
            load_hours: hours(l.load_minutes),
            unestimated: l.unestimated,
            over_capacity: l.load_minutes > l.capacity_minutes,
        })
        .collect();

    let mut open: Vec<&Task> = tasks
        .iter()
        .copied()
        .filter(|t| t.status.is_open())
        .collect();
    sort_by_due(&mut open);

    // Planned for this week or carried over from an earlier day.
    let planned_this_week = |t: &Task| t.planned_for.is_some_and(|d| d <= week_end);
    let to_plan = |t: &Task| {
        let days = t.due_on.map(|d| days_between(today, d));
        PlanTask {
            id: t.id,
            title: clean_title(&t.title),
            project: t.project.clone(),
            due_on: t.due_on,
            due_weekday: t.due_on.map(|d| weekday_name(d.weekday())),
            days_until_due: days,
            overdue: days.is_some_and(|n| n < 0),
            planned_for: t.planned_for,
            on_today_list: t.on_today_list(today),
            planned_weekday: t.planned_for.map(|d| weekday_name(d.weekday())),
            estimate_minutes: t.estimate_minutes,
            estimate_hours: t.estimate_minutes.map(hours),
            priority: t.priority,
            deadline_moves: net_deadline_moves(history.of(t.id)),
            notes: trim_notes(&t.notes),
        }
    };

    let mut overdue = Vec::new();
    let mut due_this_week = Vec::new();
    let mut due_soon = Vec::new();
    let mut undated_tasks: Vec<&Task> = Vec::new();
    for t in &open {
        match t.due_on {
            Some(d) if d < today => overdue.push(to_plan(t)),
            Some(d) if d <= week_end => due_this_week.push(to_plan(t)),
            Some(d) if d <= horizon || planned_this_week(t) => due_soon.push(to_plan(t)),
            Some(_) => {}
            None => undated_tasks.push(t),
        }
    }
    // Tasks on today's list are always listed, so they can be recommended;
    // the cap applies to the rest. Stable: today's list first, then planned
    // this week, list order otherwise.
    let on_today_list = |t: &Task| t.on_today_list(today);
    undated_tasks.sort_by_key(|t| (!on_today_list(t), !planned_this_week(t)));
    let on_today = undated_tasks.iter().filter(|t| on_today_list(t)).count();
    let undated: Vec<PlanTask> = undated_tasks
        .iter()
        .take(on_today + MAX_UNDATED)
        .map(|t| to_plan(t))
        .collect();

    let due: Vec<&PlanTask> = overdue.iter().chain(&due_this_week).collect();
    let due_estimate_minutes = due
        .iter()
        .filter_map(|t| t.estimate_minutes)
        .fold(0u32, u32::saturating_add);
    let available_minutes = remaining_working_days
        .iter()
        .map(|d| d.capacity_minutes)
        .fold(0u32, u32::saturating_add);

    let totals = PlanTotals {
        open: open.len(),
        overdue: overdue.len(),
        due_this_week: due_this_week.len(),
        due_soon: due_soon.len(),
        undated: undated_tasks.len(),
        due_estimate_minutes,
        due_estimate_hours: hours(due_estimate_minutes),
        available_minutes,
        available_hours: hours(available_minutes),
        due_without_estimate: due.iter().filter(|t| t.estimate_minutes.is_none()).count(),
        without_estimate: open.iter().filter(|t| t.estimate_minutes.is_none()).count(),
        overloaded: due_estimate_minutes > available_minutes,
    };
    PlanFacts {
        today,
        today_weekday: weekday_name(today.weekday()),
        week_start: week.start,
        week_end,
        remaining_working_days,
        overdue,
        due_this_week,
        due_soon,
        undated,
        totals,
    }
}

pub fn plan_facts_markdown(facts: &PlanFacts) -> String {
    let t = &facts.totals;
    let days = facts.remaining_working_days.len();
    let unestimated = match t.due_without_estimate {
        0 => String::new(),
        1 => ", 1 task has no estimate".to_string(),
        n => format!(", {n} tasks have no estimate"),
    };
    let overloaded = if t.overloaded {
        " · **overloaded**"
    } else {
        ""
    };
    let mut md = format!(
        "**Today:** {} · week {} to {}\n\n\
         **Due by {}:** {} estimated{unestimated} · **available:** {} over {days} working {}{overloaded}\n\n\
         **Open:** {} · overdue {} · due this week {} · no due date {} · no estimate {}\n",
        short_date(facts.today),
        short_date(facts.week_start),
        short_date(facts.week_end),
        short_date(facts.week_end),
        fmt_hours(t.due_estimate_minutes),
        fmt_hours(t.available_minutes),
        if days == 1 { "day" } else { "days" },
        t.open,
        t.overdue,
        t.due_this_week,
        t.undated,
        t.without_estimate,
    );

    section(
        &mut md,
        "Working days left",
        days,
        "No working days left this week.",
    );
    for d in &facts.remaining_working_days {
        let over = if d.over_capacity {
            " · over capacity"
        } else {
            ""
        };
        let unestimated = match d.unestimated {
            0 => String::new(),
            n => format!(" · {n} without estimate"),
        };
        let _ = writeln!(
            md,
            "- {}: {} planned of {}{over}{unestimated}",
            short_date(d.date),
            fmt_hours(d.load_minutes),
            fmt_hours(d.capacity_minutes)
        );
    }

    let groups: [(&str, &[PlanTask], &str); 3] = [
        ("Overdue", &facts.overdue, "Nothing overdue."),
        (
            "Due this week",
            &facts.due_this_week,
            "Nothing due this week.",
        ),
        (
            "Due in the next two weeks",
            &facts.due_soon,
            "Nothing else due in the next two weeks.",
        ),
    ];
    for (title, tasks, empty) in groups {
        section(&mut md, title, tasks.len(), empty);
        for task in tasks {
            let (Some(due), Some(n)) = (task.due_on, task.days_until_due) else {
                continue;
            };
            let _ = writeln!(
                md,
                "- {}{} · due {} ({}){}",
                task.title,
                project_suffix(&task.project),
                short_date(due),
                relative_days(n),
                plan_task_details(task)
            );
        }
    }

    let heading = if t.undated > facts.undated.len() {
        format!(
            "No due date (showing {} of {})",
            facts.undated.len(),
            t.undated
        )
    } else {
        "No due date".to_string()
    };
    section(
        &mut md,
        &heading,
        facts.undated.len(),
        "Every open task has a due date.",
    );
    for task in &facts.undated {
        let _ = writeln!(
            md,
            "- {}{}{}",
            task.title,
            project_suffix(&task.project),
            plan_task_details(task)
        );
    }
    md
}

/// " · ~2h · !! · planned Thu 19 Nov · deadline moved 2×", each part only
/// when it applies.
pub(super) fn plan_task_details(task: &PlanTask) -> String {
    let mut s = String::new();
    if let Some(m) = task.estimate_minutes {
        let _ = write!(s, " · ~{}", fmt_hours(m));
    }
    if task.priority > 0 {
        let _ = write!(s, " · {}", "!".repeat(usize::from(task.priority)));
    }
    if let Some(p) = task.planned_for.filter(|p| Some(*p) != task.due_on) {
        let _ = write!(s, " · planned {}", short_date(p));
    }
    match task.deadline_moves {
        0 => {}
        1 => s.push_str(" · deadline moved once"),
        n => {
            let _ = write!(s, " · deadline moved {n}×");
        }
    }
    s
}

/// True when there is nothing to recommend: no open task is on the Today
/// list (planned for today or carried over) and none is overdue.
/// Recommendations only ever come from those.
pub fn plan_is_empty(facts: &PlanFacts) -> bool {
    facts.overdue.is_empty()
        && !facts
            .due_this_week
            .iter()
            .chain(&facts.due_soon)
            .chain(&facts.undated)
            .any(|t| t.on_today_list)
}

/// The built-in system prompt for recommended tasks. The user can replace it
/// in Settings (`Settings::plan_prompt`); `DATA_RULE` and `PLAN_FORMAT` are
/// always added, so a custom prompt can't break the reply format.
pub const DEFAULT_PLAN_PROMPT: &str = "\
You pick the three tasks the user should do first today (`today`), most important first, from the tasks on today's list (`on_today_list`: planned for today, or for an earlier day and carried over) and the overdue tasks (`overdue`: due on an earlier day and still not done). Be direct and practical.

- Use only the facts provided. Do not invent tasks, deadlines or estimates.
- Do no arithmetic: every date, day count and hour figure is already computed.
- Weigh priority, deadlines and context together. Higher `priority` comes first (2 or more high, 1 medium, 0 low). An overdue task stays a candidate every day until it is done; the longer it is overdue, the more it should rise. Among tasks of the same priority, overdue tasks and the nearest deadlines come first.
- Use the notes, project and estimate for context: a task that unblocks others or that people are waiting on comes earlier.
- Prefer what fits in today's `capacity_hours` (the first of `remaining_working_days`).
- Each reason is a few words, such as \"due Thursday\" or \"overdue 3 days\".";

/// The reply format for recommended tasks, parsed by `parse_recommendations`.
pub(super) const PLAN_FORMAT: &str = "\
Reply with one JSON object and nothing else (no code fences, no prose):
{\"tasks\": [{\"id\": <a task id from the facts>, \"why\": \"<a few words>\"}]}
List at most 3 tasks, the most important first. Use only ids that appear in the facts.
Pick only tasks whose `on_today_list` is true or that are in `overdue`: never another task. If fewer than 3 are, list only those.";

/// The recommended-tasks prompt in use: the user's, or the built-in one.
pub fn plan_prompt(settings: &Settings) -> &str {
    match settings.plan_prompt.trim() {
        "" => DEFAULT_PLAN_PROMPT,
        custom => custom,
    }
}

pub(super) fn plan_system(settings: &Settings) -> String {
    format!("{}\n\n{DATA_RULE}\n\n{PLAN_FORMAT}", plan_prompt(settings))
}

pub fn prepare_plan(
    tasks: &[Task],
    events: &[Event],
    week: WeekRef,
    today: Date,
    settings: &Settings,
) -> Prepared {
    let facts = plan_facts(tasks, events, week, today, settings);
    let digest_json = facts_json(&facts);
    let user = format!(
        "Pick the three tasks I should do first today, {}. This week runs from {} to {}.\n\n<facts>\n{digest_json}\n</facts>",
        long_date(facts.today),
        long_date(facts.week_start),
        long_date(facts.week_end),
    );
    Prepared {
        kind: ReportKind::Plan,
        week,
        digest_json,
        request: ChatRequest {
            system: plan_system(settings),
            user,
        },
    }
}

/// Estimated load on one day against its capacity. For the Week tab's load
/// bars and the plan facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DayLoad {
    pub date: Date,
    /// Sum of `estimate_minutes` of open tasks whose `Task::day()` is this
    /// date.
    pub load_minutes: u32,
    /// `Settings::focus_minutes_per_day` on working days, 0 otherwise.
    pub capacity_minutes: u32,
    /// Open tasks on this day with no estimate.
    pub unestimated: usize,
}

/// One `DayLoad` per day of `week`, in order (seven entries). Private tasks
/// count: the load is shown locally and never sent anywhere.
pub fn day_loads(tasks: &[Task], week: WeekRef, settings: &Settings) -> Vec<DayLoad> {
    loads(tasks, week, settings)
}

pub(super) fn loads<'a>(
    tasks: impl IntoIterator<Item = &'a Task>,
    week: WeekRef,
    settings: &Settings,
) -> Vec<DayLoad> {
    let working = &settings.schedule.working_days;
    let mut out: Vec<DayLoad> = week
        .days()
        .into_iter()
        .map(|date| DayLoad {
            date,
            load_minutes: 0,
            capacity_minutes: if working.contains(&date.weekday()) {
                settings.focus_minutes_per_day
            } else {
                0
            },
            unestimated: 0,
        })
        .collect();
    for t in tasks.into_iter().filter(|t| t.status.is_open()) {
        let Some(day) = t.day() else { continue };
        let Some(load) = out.iter_mut().find(|l| l.date == day) else {
            continue;
        };
        match t.estimate_minutes {
            Some(m) => load.load_minutes = load.load_minutes.saturating_add(m),
            None => load.unestimated += 1,
        }
    }
    out
}

/// The working-day capacity a column without one borrows, when the week
/// has no working day with focus time.
pub(super) const FALLBACK_CAPACITY: u32 = 8 * 60;

/// One Week-tab column's load bar: its tasks against its capacity. Tasks
/// with an estimate take their estimate; tasks without one share what the
/// estimates leave of the capacity evenly (nothing left: they take
/// nothing). Open and done tasks both count; deleted ones don't.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnLoad {
    /// The column's capacity, at least 1 (see `column_load`).
    pub capacity_minutes: u32,
    /// Sum of the estimates of the column's tasks.
    pub estimated_minutes: u32,
    /// Tasks with no estimate.
    pub untimed: usize,
    /// Each untimed task's share: what the estimates leave free, split
    /// evenly. 0 with no untimed task or no time left.
    pub untimed_share: f32,
    /// Everything planned: the estimates plus the untimed shares.
    pub planned_minutes: f32,
    /// The part of `planned_minutes` that is done.
    pub done_minutes: f32,
    /// Estimates booked beyond capacity, when there are any.
    pub over_by: Option<u32>,
}

impl ColumnLoad {
    /// Everything planned over capacity, 0–1.
    pub fn fraction(&self) -> f32 {
        (self.planned_minutes / self.capacity_minutes as f32).min(1.0)
    }

    /// The done part over capacity, 0–1 (at most `fraction`).
    pub fn done_fraction(&self) -> f32 {
        (self.done_minutes / self.capacity_minutes as f32).min(1.0)
    }
}

/// The load of a Week-tab column listing `days`, from the week's
/// `day_loads`. Its tasks are those whose `Task::day()` is one of `days`,
/// open or done (not dropped); private tasks count, as in `day_loads`.
///
/// Capacity: the sum of the days' `capacity_minutes`. A column with none
/// (the shared non-working-days column) borrows a working day's capacity
/// (the largest in `loads`, else 8h) for each of its days that has tasks,
/// at least one.
pub fn column_load(tasks: &[Task], days: &[Date], loads: &[DayLoad]) -> ColumnLoad {
    let on = |d: &Date| {
        let d = *d;
        tasks
            .iter()
            .filter(move |t| t.day() == Some(d) && t.status != TaskStatus::Dropped)
    };
    let own: u32 = loads
        .iter()
        .filter(|l| days.contains(&l.date))
        .map(|l| l.capacity_minutes)
        .fold(0, u32::saturating_add);
    let capacity = if own > 0 {
        own
    } else {
        let nominal = loads
            .iter()
            .map(|l| l.capacity_minutes)
            .max()
            .filter(|m| *m > 0)
            .unwrap_or(FALLBACK_CAPACITY);
        let busy = days
            .iter()
            .filter(|d| on(d).next().is_some())
            .count()
            .max(1);
        nominal.saturating_mul(u32::try_from(busy).unwrap_or(u32::MAX))
    }
    .max(1);

    let column: Vec<&Task> = days.iter().flat_map(on).collect();
    let estimated: u32 = column
        .iter()
        .filter_map(|t| t.estimate_minutes)
        .fold(0, u32::saturating_add);
    let untimed = column
        .iter()
        .filter(|t| t.estimate_minutes.is_none())
        .count();
    let untimed_share = match untimed {
        0 => 0.0,
        n => capacity.saturating_sub(estimated) as f32 / n as f32,
    };
    let minutes = |t: &Task| t.estimate_minutes.map_or(untimed_share, |m| m as f32);
    ColumnLoad {
        capacity_minutes: capacity,
        estimated_minutes: estimated,
        untimed,
        untimed_share,
        planned_minutes: column.iter().map(|t| minutes(t)).sum(),
        done_minutes: column
            .iter()
            .filter(|t| t.status == TaskStatus::Done)
            .map(|t| minutes(t))
            .sum(),
        over_by: (estimated > capacity).then(|| estimated - capacity),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Settings, Task, TaskStatus, WeekRef};
    use crate::report::fixtures::*;
    use jiff::civil::Weekday;
    use jiff::civil::date;

    // --- plan facts --------------------------------------------------------

    #[test]
    fn plan_is_empty_unless_a_task_is_on_the_today_list_or_overdue() {
        let monday = date(2026, 11, 16);
        let week = WeekRef::new(monday);
        let settings = Settings::default();
        let elsewhere = vec![
            due(task(1, "Wednesday's"), date(2026, 11, 18)),
            task(2, "No day"),
        ];
        let facts = plan_facts(&elsewhere, &[], week, monday, &settings);
        assert!(plan_is_empty(&facts));
        let mut today = elsewhere.clone();
        today.push(due(task(3, "Today's"), monday));
        let facts = plan_facts(&today, &[], week, monday, &settings);
        assert!(!plan_is_empty(&facts));
        // An overdue task is a candidate too, whatever day it sits on.
        let mut late = elsewhere;
        late.push(due(task(4, "Last week's"), date(2026, 11, 12)));
        let facts = plan_facts(&late, &[], week, monday, &settings);
        assert!(!plan_is_empty(&facts));
        // So is a task carried over from an earlier day, even with a far
        // deadline or none.
        for deadline in [Some(date(2026, 12, 10)), None] {
            let mut carried = vec![planned(task(5, "Friday's"), date(2026, 11, 13))];
            carried[0].due_on = deadline;
            let facts = plan_facts(&carried, &[], week, monday, &settings);
            assert!(!plan_is_empty(&facts), "{deadline:?}");
        }
    }

    #[test]
    fn plan_buckets_overdue_this_week_and_soon() {
        let f = plan_facts(
            &plan_fixture(),
            &plan_events(),
            plan_week(),
            plan_today(),
            &Settings::default(),
        );
        let ids = |v: &[PlanTask]| v.iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(ids(&f.overdue), vec![1]);
        assert_eq!(ids(&f.due_this_week), vec![2, 3, 4]);
        assert_eq!(ids(&f.due_soon), vec![5, 6, 11]);
        assert_eq!(ids(&f.undated), vec![8, 9]);
        assert_eq!(f.overdue[0].days_until_due, Some(-6));
        assert!(f.overdue[0].overdue);
        assert_eq!(f.due_this_week[0].days_until_due, Some(0));
        assert!(!f.due_this_week[0].overdue);
        assert_eq!(f.due_this_week[2].due_weekday, Some("Sunday"));
        let deck = &f.due_this_week[1];
        assert_eq!(deck.estimate_minutes, Some(240));
        assert_eq!(deck.estimate_hours, Some(4.0));
        assert_eq!(deck.priority, 2);
        assert_eq!(deck.planned_for, Some(date(2026, 11, 19)));
        assert_eq!(deck.planned_weekday, Some("Thursday"));
        assert_eq!(
            f.totals,
            PlanTotals {
                open: 10,
                overdue: 1,
                due_this_week: 3,
                due_soon: 3,
                undated: 2,
                due_estimate_minutes: 420,
                due_estimate_hours: 7.0,
                available_minutes: 1440,
                available_hours: 24.0,
                due_without_estimate: 1,
                without_estimate: 6,
                overloaded: false,
            }
        );
    }

    #[test]
    fn deadline_moves_count_later_moves_all_time() {
        let f = plan_facts(
            &plan_fixture(),
            &plan_events(),
            plan_week(),
            plan_today(),
            &Settings::default(),
        );
        let moves: Vec<(i64, usize)> = f
            .overdue
            .iter()
            .chain(&f.due_this_week)
            .chain(&f.due_soon)
            .map(|t| (t.id, t.deadline_moves))
            .collect();
        assert_eq!(
            moves,
            vec![(1, 1), (2, 0), (3, 2), (4, 0), (5, 0), (6, 0), (11, 0)]
        );
    }

    #[test]
    fn remaining_days_carry_capacity_and_load_without_private_tasks() {
        let f = plan_facts(
            &plan_fixture(),
            &plan_events(),
            plan_week(),
            plan_today(),
            &Settings::default(),
        );
        let days: Vec<(&str, u32, u32, usize, bool)> = f
            .remaining_working_days
            .iter()
            .map(|d| {
                (
                    d.weekday,
                    d.capacity_minutes,
                    d.load_minutes,
                    d.unestimated,
                    d.over_capacity,
                )
            })
            .collect();
        // Thursday: the board deck (planned) but not the private appointment.
        assert_eq!(
            days,
            vec![
                ("Wednesday", 480, 120, 0, false),
                ("Thursday", 480, 240, 0, false),
                ("Friday", 480, 90, 0, false),
            ]
        );
        assert_eq!(f.remaining_working_days[2].load_hours, 1.5);
    }

    #[test]
    fn overloaded_when_due_work_exceeds_the_time_left() {
        let s = Settings {
            focus_minutes_per_day: 120,
            ..Settings::default()
        };
        // Friday: one working day of two hours left.
        let fri = date(2026, 11, 20);
        let tasks = vec![
            estimate(due(task(1, "Big"), fri), 150),
            due(task(2, "Unknown"), fri),
            estimate(due(task(3, "Late"), date(2026, 11, 10)), 30),
        ];
        let f = plan_facts(&tasks, &[], plan_week(), fri, &s);
        let t = &f.totals;
        assert_eq!(t.due_estimate_minutes, 180);
        assert_eq!(t.due_estimate_hours, 3.0);
        assert_eq!(t.available_minutes, 120);
        assert_eq!(t.available_hours, 2.0);
        assert_eq!(t.due_without_estimate, 1);
        assert!(t.overloaded);
        assert!(f.remaining_working_days[0].over_capacity);
        assert_eq!(f.remaining_working_days[0].unestimated, 1);
        assert!(plan_facts_markdown(&f).contains("**overloaded**"));

        // Saturday: nothing left, anything due is overloaded.
        let f = plan_facts(&tasks, &[], plan_week(), date(2026, 11, 21), &s);
        assert_eq!(f.totals.available_minutes, 0);
        assert!(f.totals.overloaded);
        // Nothing estimated and no time: not overloaded.
        let f = plan_facts(&[], &[], plan_week(), date(2026, 11, 21), &s);
        assert!(!f.totals.overloaded);
    }

    #[test]
    fn remaining_working_days_follow_settings() {
        let days = |today: Date, s: &Settings| {
            plan_facts(&[], &[], plan_week(), today, s)
                .remaining_working_days
                .iter()
                .map(|d| d.weekday)
                .collect::<Vec<_>>()
        };
        let default = Settings::default();
        assert_eq!(
            days(plan_today(), &default),
            vec!["Wednesday", "Thursday", "Friday"]
        );
        // Before the week starts: every working day of the week.
        assert_eq!(days(date(2026, 11, 15), &default).len(), 5);
        // Saturday: none left.
        assert!(days(date(2026, 11, 21), &default).is_empty());

        let mut custom = Settings::default();
        custom.schedule.working_days = vec![Weekday::Monday, Weekday::Thursday, Weekday::Sunday];
        assert_eq!(days(plan_today(), &custom), vec!["Thursday", "Sunday"]);
    }

    #[test]
    fn plan_facts_read_back_as_sent() {
        let f = plan_facts(
            &plan_fixture(),
            &plan_events(),
            plan_week(),
            plan_today(),
            &Settings::default(),
        );
        let back: PlanFacts = serde_json::from_str(&facts_json(&f)).unwrap();
        assert_eq!(back, f);
        let bad = facts_json(&f).replace("\"Wednesday\"", "\"Caturday\"");
        assert!(serde_json::from_str::<PlanFacts>(&bad).is_err());
    }

    #[test]
    fn undated_is_capped_but_counted() {
        let mut tasks: Vec<Task> = (1..=14).map(|i| task(i, "Undated")).collect();
        // Planned for this week: listed first despite its position.
        tasks[13] = planned(task(14, "Planned"), date(2026, 11, 19));
        let f = plan_facts(&tasks, &[], plan_week(), plan_today(), &Settings::default());
        assert_eq!(f.undated.len(), MAX_UNDATED);
        assert_eq!(f.undated[0].id, 14);
        assert_eq!(f.totals.undated, 14);
        assert!(plan_facts_markdown(&f).contains("No due date (showing 10 of 14)"));

        // Tasks on today's list are never cut, however many others are
        // planned for later this week and sit earlier in the list.
        let mut tasks: Vec<Task> = (1..=12)
            .map(|i| planned(task(i, "Later this week"), date(2026, 11, 19)))
            .collect();
        tasks.push(planned(task(13, "Today"), plan_today()));
        tasks.push(planned(task(14, "Carried over"), date(2026, 11, 16)));
        tasks.push(task(15, "Someday"));
        let f = plan_facts(&tasks, &[], plan_week(), plan_today(), &Settings::default());
        assert_eq!(f.undated.len(), MAX_UNDATED + 2);
        assert_eq!(f.undated[0].id, 13);
        assert_eq!(f.undated[1].id, 14);
        assert!(f.undated[2..].iter().all(|t| !t.on_today_list));
        assert_eq!(f.totals.undated, 15);
        assert!(!plan_is_empty(&f));
    }

    // --- capacity ----------------------------------------------------------

    #[test]
    fn day_loads_sum_open_estimates_by_day() {
        let mut s = Settings {
            focus_minutes_per_day: 300,
            ..Settings::default()
        };
        s.schedule.working_days = vec![Weekday::Monday, Weekday::Tuesday, Weekday::Saturday];
        let mon = date(2026, 11, 16);
        let tue = date(2026, 11, 17);
        let mut doing = estimate(due(task(4, "Doing"), mon), 15);
        doing.status = TaskStatus::Doing;
        let mut dropped = estimate(due(task(6, "Dropped"), mon), 500);
        dropped.status = TaskStatus::Dropped;
        let tasks = vec![
            estimate(due(task(1, "A"), mon), 60),
            estimate(due(task(2, "B"), mon), 45),
            // Planned for Tuesday, due Monday: counts on Tuesday.
            estimate(planned(due(task(3, "C"), mon), tue), 30),
            doing,
            done_at(
                estimate(due(task(5, "Done"), mon), 500),
                "2026-11-16T10:00:00Z",
            ),
            dropped,
            due(task(7, "No estimate"), mon),
            // Private tasks count: the load is local.
            estimate(private(due(task(8, "Private"), tue)), 120),
            // Outside the week, and undated: ignored.
            estimate(due(task(9, "Next week"), date(2026, 11, 23)), 60),
            estimate(task(10, "Undated"), 60),
            // Sunday, not a working day: load but no capacity.
            estimate(due(task(11, "Sunday"), date(2026, 11, 22)), 20),
        ];
        let loads = day_loads(&tasks, plan_week(), &s);
        assert_eq!(loads.len(), 7);
        let got: Vec<(Date, u32, u32, usize)> = loads
            .iter()
            .map(|l| (l.date, l.load_minutes, l.capacity_minutes, l.unestimated))
            .collect();
        assert_eq!(
            got,
            vec![
                (mon, 120, 300, 1),
                (tue, 150, 300, 0),
                (date(2026, 11, 18), 0, 0, 0),
                (date(2026, 11, 19), 0, 0, 0),
                (date(2026, 11, 20), 0, 0, 0),
                (date(2026, 11, 21), 0, 300, 0),
                (date(2026, 11, 22), 20, 0, 0),
            ]
        );
    }

    /// The same figures the Week tab's load bars showed when this lived in
    /// the view (`views/week.rs` `load_line`, `nominal_capacity`).
    #[test]
    fn column_load_shares_what_the_estimates_leave() {
        let d = date(2026, 11, 18);
        let est = |id, status, m: Option<u32>| {
            let mut t = planned(task(id, "T"), d);
            t.status = status;
            t.estimate_minutes = m;
            t
        };
        let wed = |cap| DayLoad {
            date: d,
            load_minutes: 0,
            capacity_minutes: cap,
            unestimated: 0,
        };
        let loads = [wed(480)];

        // Four tasks without a time: a quarter of the day each.
        let four: Vec<Task> = (1..=4).map(|i| est(i, TaskStatus::Todo, None)).collect();
        let l = column_load(&four, &[d], &loads);
        assert_eq!(l.capacity_minutes, 480);
        assert_eq!((l.estimated_minutes, l.untimed), (0, 4));
        assert_eq!(l.untimed_share, 120.0);
        assert_eq!((l.fraction(), l.done_fraction()), (1.0, 0.0));

        // Two 2h tasks and two without a time: 25% each; one of each done.
        // A dropped task and another day's don't count.
        let mut dropped = est(5, TaskStatus::Dropped, Some(600));
        dropped.planned_for = Some(d);
        let mixed = vec![
            est(1, TaskStatus::Done, Some(120)),
            est(2, TaskStatus::Todo, Some(120)),
            est(3, TaskStatus::Done, None),
            est(4, TaskStatus::Todo, None),
            dropped,
            planned(estimate(task(6, "Thursday"), 600), date(2026, 11, 19)),
        ];
        let l = column_load(&mixed, &[d], &loads);
        assert_eq!((l.fraction(), l.done_fraction()), (1.0, 0.5));
        assert_eq!(l.over_by, None);

        // Only timed tasks: their share, nothing more.
        let l = column_load(&mixed[..2], &[d], &loads);
        assert_eq!((l.fraction(), l.done_fraction()), (0.5, 0.25));
        assert_eq!((l.estimated_minutes, l.done_minutes), (240, 120.0));

        // Estimates over capacity: untimed tasks get nothing, the bar is
        // full and the overrun shows.
        let over = vec![
            est(1, TaskStatus::Todo, Some(540)),
            est(2, TaskStatus::Done, None),
        ];
        let l = column_load(&over, &[d], &loads);
        assert_eq!((l.fraction(), l.done_fraction()), (1.0, 0.0));
        assert_eq!(l.over_by, Some(60));
        assert_eq!((l.untimed, l.untimed_share), (1, 0.0));

        // No tasks: an empty bar.
        let l = column_load(&[], &[d], &loads);
        assert_eq!((l.fraction(), l.done_fraction()), (0.0, 0.0));
        assert_eq!(l.capacity_minutes, 480);

        // The non-working column borrows a working day's capacity (the
        // largest, 7h here) for each of its days with tasks; 8h without
        // any working day.
        let (sat, sun) = (date(2026, 11, 21), date(2026, 11, 22));
        let weekend = |cap| {
            [
                wed(cap),
                DayLoad {
                    date: sat,
                    capacity_minutes: 0,
                    ..wed(0)
                },
                DayLoad {
                    date: sun,
                    capacity_minutes: 0,
                    ..wed(0)
                },
            ]
        };
        let one = vec![planned(estimate(task(1, "Sat"), 210), sat)];
        let l = column_load(&one, &[sat, sun], &weekend(420));
        assert_eq!(l.capacity_minutes, 420);
        assert_eq!(l.fraction(), 0.5);
        let both = vec![
            planned(estimate(task(1, "Sat"), 210), sat),
            planned(task(2, "Sun"), sun),
        ];
        let l = column_load(&both, &[sat, sun], &weekend(420));
        assert_eq!(l.capacity_minutes, 840);
        assert_eq!(l.untimed_share, 630.0);
        assert_eq!(
            column_load(&[], &[sat, sun], &weekend(0)).capacity_minutes,
            480
        );
        assert_eq!(column_load(&both, &[sat, sun], &[]).capacity_minutes, 960);
    }

    #[test]
    fn snapshot_plan() {
        let tasks = plan_fixture();
        let events = plan_events();
        let s = Settings::default();
        let facts = plan_facts(&tasks, &events, plan_week(), plan_today(), &s);
        let p = prepare_plan(&tasks, &events, plan_week(), plan_today(), &s);
        snap_settings().bind(|| {
            insta::assert_json_snapshot!("report__plan_facts", facts);
            insta::assert_snapshot!("report__plan_markdown", plan_facts_markdown(&facts));
            insta::assert_snapshot!("report__plan_system", p.request.system);
            insta::assert_snapshot!("report__plan_user", p.request.user);
        });
    }

    #[test]
    fn snapshot_empty_week() {
        let s = Settings::default();
        let plan = plan_facts(&[], &[], plan_week(), plan_today(), &s);
        snap_settings().bind(|| {
            insta::assert_json_snapshot!("report__empty_plan_facts", plan);
            insta::assert_snapshot!("report__empty_plan_markdown", plan_facts_markdown(&plan));
        });
    }
}
