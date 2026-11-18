//! Data flow: reloading the tasks and events after a change, the cache of
//! values worked out from them (`Derived`, cleared by `invalidate`), and
//! pushing fresh data into the visible tab and Settings (`refresh`).

use crate::Ready;
use fortnight_app::top_bar::Tab;
use fortnight_app::views::day::{self, DayData, LogLine};
use fortnight_app::views::month::MonthData;
use fortnight_app::views::settings::SettingsData;
use fortnight_app::views::week::WeekData;
use fortnight_core::domain::{Report, ReportKey, ReportKind};
use fortnight_core::report;
use fortnight_core::store::Store;
use jiff::Timestamp;
use std::collections::HashMap;

/// Values worked out from the tasks, events, reports and settings, kept
/// until `Ready::invalidate`.
#[derive(Default)]
pub(crate) struct Derived {
    /// Every version of a report, newest first.
    pub(super) versions: HashMap<ReportKey, Vec<Report>>,
    /// Whether a report has nothing to work from, and its facts' digest.
    pub(super) facts: HashMap<ReportKey, (bool, String)>,
    pub(super) recommended_today: Option<Vec<day::Recommended>>,
    logs: Option<Vec<LogLine>>,
}

impl Ready {
    pub(super) fn reload_tasks(&mut self) {
        self.tasks_changed = true;
        self.invalidate();
        match self.store.list_tasks() {
            Ok(tasks) => self.tasks = tasks,
            Err(e) => self.error = Some(e.to_string()),
        }
        match self.store.events_between(Timestamp::MIN, Timestamp::MAX) {
            Ok(events) => self.events = events,
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Forgets every derived value (tasks, events, reports or settings
    /// changed, or the minute ticked) and has the views refreshed.
    pub(crate) fn invalidate(&mut self) {
        self.derived = Derived::default();
        self.must_refresh = true;
    }

    /// Pushes fresh data into the visible tab (and Settings when open).
    pub(crate) fn refresh(&mut self) {
        self.must_refresh = false;
        let today = self.today();
        match self.tab {
            Tab::Today => {
                let (recommended, recommending) = self.day_recommendations(today);
                let logs = self.logs();
                let day_loads =
                    report::day_loads(&self.tasks, self.week_of(self.selected), &self.settings);
                let load = report::column_load(&self.tasks, &[self.selected], &day_loads);
                self.day_view.load(DayData {
                    day: self.selected,
                    today,
                    tz: self.tz.clone(),
                    tasks: self.tasks.clone(),
                    week_start: self.settings.schedule.week_start,
                    recommended,
                    recommending,
                    logs,
                    recommend_overdue: self.settings.recommend_overdue,
                    load: Some(load),
                });
            }
            Tab::Week => {
                let week = self.week_of(self.selected);
                let panel = self.panel_data(week);
                let data = WeekData {
                    week,
                    today,
                    tasks: self.tasks.clone(),
                    working_days: self.settings.schedule.working_days.clone(),
                    loads: report::day_loads(&self.tasks, week, &self.settings),
                    panel,
                };
                self.week_data = Some(data.clone());
                self.week_view.load(data);
            }
            Tab::Month => {
                let summary_weeks = match self.store.ready_report_keys() {
                    Ok(keys) => keys
                        .into_iter()
                        .filter(|k| k.kind == ReportKind::Summary)
                        .map(|k| k.week_start)
                        .collect(),
                    Err(e) => {
                        self.error = Some(e.to_string());
                        Vec::new()
                    }
                };
                self.month_view.load(MonthData {
                    month: self.selected.first_of_month(),
                    today,
                    week_start: self.settings.schedule.week_start,
                    working_days: self.settings.schedule.working_days.clone(),
                    tasks: self.tasks.clone(),
                    summary_weeks,
                });
            }
        }

        if self.settings_open {
            self.settings_view.load(SettingsData {
                settings: self.settings.clone(),
                data_dir: self.data_dir.clone(),
                key_state: self.keys.key_state(),
                decision_key_state: self.keys.decision_key_state(),
                test_result: self.test_result.clone(),
                testing: self.testing,
                key_message: self.key_message.clone(),
                models: self.models.clone(),
                ollama: self.ollama.clone(),
            });
        }
    }

    /// The free-standing log lines (day notes), each on its local date.
    fn logs(&mut self) -> Vec<LogLine> {
        if let Some(logs) = &self.derived.logs {
            return logs.clone();
        }
        let logs: Vec<LogLine> = self
            .events
            .iter()
            .filter(|e| e.task_id.is_none())
            .filter_map(|e| {
                Some(LogLine {
                    date: self.tz.to_datetime(e.at).date(),
                    text: e.log_text()?.to_string(),
                })
            })
            .collect();
        self.derived.logs = Some(logs.clone());
        logs
    }
}
