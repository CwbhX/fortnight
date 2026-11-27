//! Quick Search & Project Filter overlay (`⌘F` or `/`).
//!
//! Lets the user search across tasks (title, project, notes) and day notes
//! (`LogLine`), filter by `#project` chip, toggle between open tasks and all
//! tasks (including completed & deleted), and jump straight to a task or day
//! with the keyboard or mouse.

use std::cmp::Ordering;

use fortnight_core::domain::{Task, TaskId, TaskStatus};
use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, Space, button, column, container, operation, row, rule, scrollable, text,
    text_input,
};
use iced::{Border, Center, Color, Element, Fill, Padding, Theme, border};
use jiff::civil::Date;

use crate::theme::{self, Icon};
use crate::views::day::LogLine;

/// The id of the search input, for `operation::focus`.
pub const SEARCH_INPUT_ID: &str = "search-input";

/// Maximum number of results shown in the scrollable list.
const MAX_RESULTS: usize = 60;

#[derive(Debug, Clone)]
pub enum Message {
    QueryChanged(String),
    /// Click a project chip (`None` = "All", `Some(name)` = toggle that project).
    SelectProject(Option<String>),
    /// Toggle between Open (`false`) and All (`true`, includes done & deleted).
    SetIncludeAll(bool),
    /// Click a result row at this index.
    Activate(usize),
    /// Press Enter in the search input: activate the currently selected row.
    ActivateSelected,
    Close,
}

/// What `main.rs` must do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    OpenTask { id: TaskId, day: Date },
    OpenDay(Date),
}

/// Everything the overlay searches over, supplied by `main.rs`.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchData {
    pub tasks: Vec<Task>,
    pub logs: Vec<LogLine>,
    pub today: Date,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SearchResult {
    Task {
        id: TaskId,
        day: Date,
        title: String,
        status: TaskStatus,
        project: Option<String>,
        date_label: Option<String>,
        overdue: bool,
        priority: u8,
    },
    DayNote {
        date: Date,
        snippet: String,
    },
}

#[derive(Debug, Default)]
pub struct SearchView {
    data: Option<SearchData>,
    query: String,
    selected_project: Option<String>,
    include_all: bool,
    selected: usize,
    /// Distinct projects sorted case-insensitively, rebuilt in `load`.
    projects: Vec<String>,
    /// Current filtered results, rebuilt when data or filters change.
    results: Vec<SearchResult>,
}

impl SearchView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Focuses the search input when the overlay opens.
    pub fn focus() -> iced::Task<Message> {
        operation::focus(SEARCH_INPUT_ID)
    }

    /// Replaces the data being searched. Cheap when `data` is unchanged.
    pub fn load(&mut self, data: SearchData) {
        if self.data.as_ref() == Some(&data) {
            return;
        }
        self.projects = distinct_projects(&data.tasks);
        // Clear the project filter if that project no longer exists.
        if let Some(selected) = &self.selected_project
            && !self
                .projects
                .iter()
                .any(|p| p.eq_ignore_ascii_case(selected))
        {
            self.selected_project = None;
        }
        self.data = Some(data);
        self.recompute();
    }

    /// Clears the query, filters, and selection (e.g. when reopening).
    pub fn reset(&mut self) {
        self.query.clear();
        self.selected_project = None;
        self.include_all = false;
        self.selected = 0;
        self.recompute();
    }

    /// Moves the keyboard selection highlight up (`delta < 0`) or down
    /// (`delta > 0`), clamped to the current results.
    pub fn move_selection(&mut self, delta: i32) {
        if self.results.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.results.len() - 1;
        let next = (self.selected as isize + delta as isize).clamp(0, last as isize) as usize;
        self.selected = next;
    }

    /// Returns the action for the currently selected result, if any.
    pub fn activate_selected(&self) -> Action {
        self.action_for(self.selected)
    }

    pub fn update(&mut self, message: Message) -> Action {
        match message {
            Message::QueryChanged(q) => {
                self.query = q;
                self.selected = 0;
                self.recompute();
                Action::None
            }
            Message::SelectProject(project) => {
                self.selected_project = match (&self.selected_project, project) {
                    (Some(cur), Some(next)) if cur.eq_ignore_ascii_case(&next) => None,
                    (_, next) => next,
                };
                self.selected = 0;
                self.recompute();
                Action::None
            }
            Message::SetIncludeAll(all) => {
                if self.include_all != all {
                    self.include_all = all;
                    self.selected = 0;
                    self.recompute();
                }
                Action::None
            }
            Message::Activate(index) => self.action_for(index),
            Message::ActivateSelected => self.activate_selected(),
            Message::Close => Action::Close,
        }
    }

    fn action_for(&self, index: usize) -> Action {
        match self.results.get(index) {
            Some(SearchResult::Task { id, day, .. }) => Action::OpenTask { id: *id, day: *day },
            Some(SearchResult::DayNote { date, .. }) => Action::OpenDay(*date),
            None => Action::None,
        }
    }

    fn recompute(&mut self) {
        self.results.clear();
        let Some(data) = &self.data else {
            self.selected = 0;
            return;
        };
        let terms: Vec<String> = self
            .query
            .split_whitespace()
            .map(|w| w.to_ascii_lowercase())
            .collect();

        let mut matched_tasks: Vec<&Task> = data
            .tasks
            .iter()
            .filter(|t| self.include_all || t.status.is_open())
            .filter(|t| match &self.selected_project {
                Some(proj) => t
                    .project
                    .as_deref()
                    .is_some_and(|p| p.eq_ignore_ascii_case(proj)),
                None => true,
            })
            .filter(|t| task_matches(t, &terms))
            .collect();

        matched_tasks.sort_by(|a, b| compare_tasks(a, b, data.today));

        for task in matched_tasks.into_iter().take(MAX_RESULTS) {
            let day = task.day().or(task.due_on).unwrap_or(data.today);
            let overdue = task.is_overdue(data.today);
            let date_label = task_date_label(task, data.today);
            self.results.push(SearchResult::Task {
                id: task.id,
                day,
                title: task.title.clone(),
                status: task.status,
                project: task.project.clone(),
                date_label,
                overdue,
                priority: task.priority,
            });
        }

        // Day notes match when there is an active search query and no project filter.
        if !terms.is_empty() && self.selected_project.is_none() && self.results.len() < MAX_RESULTS
        {
            let mut matched_logs: Vec<&LogLine> = data
                .logs
                .iter()
                .filter(|log| !log.text.trim().is_empty() && log_matches(log, &terms))
                .collect();
            // Newest day first.
            matched_logs.sort_by_key(|l| std::cmp::Reverse(l.date));
            let remaining = MAX_RESULTS - self.results.len();
            for log in matched_logs.into_iter().take(remaining) {
                self.results.push(SearchResult::DayNote {
                    date: log.date,
                    snippet: make_snippet(&log.text, &terms),
                });
            }
        }

        if self.results.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.results.len() {
            self.selected = self.results.len() - 1;
        }
    }

    /// Returns the styled `overlay_panel` container (`max_width(640)`), ready
    /// to be centered inside a backdrop in `main.rs`.
    pub fn view(&self) -> Element<'_, Message> {
        let search_input = text_input("Search tasks, notes, or #project…", &self.query)
            .id(SEARCH_INPUT_ID)
            .on_input(Message::QueryChanged)
            .on_submit(Message::ActivateSelected)
            .padding([10, 14])
            .size(theme::TEXT_BODY)
            .style(theme::input)
            .width(Fill);

        let status_toggle = container(
            row![
                button(text("Open").size(theme::TEXT_SMALL))
                    .padding([4, 10])
                    .style(theme::tab_button(!self.include_all))
                    .on_press(Message::SetIncludeAll(false)),
                button(text("All").size(theme::TEXT_SMALL))
                    .padding([4, 10])
                    .style(theme::tab_button(self.include_all))
                    .on_press(Message::SetIncludeAll(true)),
            ]
            .spacing(2),
        )
        .padding(2)
        .style(theme::tab_bar);

        let close = button(theme::icon(Icon::Close))
            .padding(theme::ICON_PADDING)
            .style(theme::icon_button)
            .on_press(Message::Close);

        let top_row = row![search_input, status_toggle, close]
            .spacing(theme::SPACING * 0.75)
            .align_y(Center);

        let mut header_col = Column::new().spacing(theme::SPACING * 0.75).push(top_row);

        if !self.projects.is_empty() {
            let mut chips = Row::new().spacing(6);
            let all_active = self.selected_project.is_none();
            chips = chips.push(
                button(text("All").size(theme::TEXT_SMALL))
                    .padding([3, 10])
                    .style(chip_button(all_active))
                    .on_press(Message::SelectProject(None)),
            );
            for project in &self.projects {
                let active = self
                    .selected_project
                    .as_deref()
                    .is_some_and(|p| p.eq_ignore_ascii_case(project));
                let name = project.clone();
                chips = chips.push(
                    button(text(format!("#{project}")).size(theme::TEXT_SMALL))
                        .padding([3, 10])
                        .style(chip_button(active))
                        .on_press(Message::SelectProject(Some(name))),
                );
            }
            header_col = header_col.push(chips.wrap());
        }

        let header = container(header_col)
            .padding(Padding {
                top: theme::SPACING * 1.25,
                bottom: theme::SPACING,
                left: theme::SPACING * 1.5,
                right: theme::SPACING * 1.25,
            })
            .width(Fill);

        let results_body: Element<'_, Message> = if self.results.is_empty() {
            let empty_msg = if self.query.trim().is_empty() && self.selected_project.is_none() {
                "No tasks to show."
            } else {
                "No matching tasks or notes."
            };
            container(text(empty_msg).size(theme::TEXT_SMALL).style(theme::muted))
                .padding([theme::SPACING * 2.0, theme::SPACING * 1.5])
                .center_x(Fill)
                .into()
        } else {
            let mut list = Column::new().spacing(4).width(Fill);
            for (i, item) in self.results.iter().enumerate() {
                let is_selected = i == self.selected;
                list = list.push(result_row(item, i, is_selected));
            }
            scrollable(list.padding(Padding {
                top: theme::SPACING * 0.75,
                bottom: theme::SPACING,
                left: theme::SPACING,
                right: theme::SPACING,
            }))
            .direction(scrollable::Direction::Vertical(
                scrollable::Scrollbar::new()
                    .width(6)
                    .scroller_width(6)
                    .margin(4),
            ))
            .height(iced::Length::Shrink)
            .into()
        };

        let footer = container(
            row![
                text("↑↓ select · Enter open · Esc close")
                    .size(12)
                    .style(theme::muted),
                Space::new().width(Fill),
                text(match self.results.len() {
                    0 => String::new(),
                    1 => "1 result".into(),
                    n if n >= MAX_RESULTS => format!("{n}+ results"),
                    n => format!("{n} results"),
                })
                .size(12)
                .style(theme::muted),
            ]
            .align_y(Center),
        )
        .padding([theme::SPACING * 0.6, theme::SPACING * 1.5])
        .width(Fill);

        let content = column![
            header,
            rule::horizontal(1).style(divider),
            container(results_body).max_height(420).width(Fill),
            rule::horizontal(1).style(divider),
            footer,
        ];

        container(content)
            .max_width(640)
            .width(Fill)
            .style(theme::overlay_panel)
            .into()
    }
}

fn result_row<'a>(item: &'a SearchResult, index: usize, selected: bool) -> Element<'a, Message> {
    let inner: Element<'a, Message> = match item {
        SearchResult::Task {
            title,
            status,
            project,
            date_label,
            overdue,
            priority,
            ..
        } => {
            let done = !status.is_open();
            let indicator: Element<'a, Message> = match status {
                TaskStatus::Done => theme::icon(Icon::Check).width(14).height(14).into(),
                TaskStatus::Dropped => text("×").size(theme::TEXT_SMALL).style(theme::muted).into(),
                TaskStatus::Todo | TaskStatus::Doing => text("○")
                    .size(theme::TEXT_SMALL)
                    .style(if *overdue {
                        theme::danger
                    } else {
                        theme::muted
                    })
                    .into(),
            };
            let title_style: fn(&Theme) -> text::Style = if done {
                theme::muted
            } else if *overdue {
                theme::danger
            } else {
                text::default
            };
            let title_el = theme::title_with_marks(
                title.as_str(),
                if done { 0 } else { *priority },
                done,
                title_style,
                |t| {
                    t.size(theme::TEXT_BODY)
                        .wrapping(Wrapping::WordOrGlyph)
                        .width(Fill)
                },
            );
            let mut r = row![container(indicator).width(16).center_x(16), title_el]
                .spacing(theme::SPACING * 0.6)
                .align_y(Center);
            if let Some(proj) = project {
                r = r.push(
                    container(text(format!("#{proj}")).size(12))
                        .padding(theme::chip_padding())
                        .style(theme::chip),
                );
            }
            if let Some(date) = date_label {
                r = r.push(
                    text(date.as_str())
                        .size(12)
                        .style(theme::due_style(*overdue && !done))
                        .wrapping(Wrapping::None),
                );
            }
            r.into()
        }
        SearchResult::DayNote { date, snippet } => {
            let badge = container(text("Note").size(11))
                .padding([2, 6])
                .style(theme::chip);
            let date_str = date.strftime("%a %-d %b").to_string();
            row![
                badge,
                text(date_str)
                    .size(theme::TEXT_SMALL)
                    .font(theme::FONT_BOLD)
                    .wrapping(Wrapping::None),
                text(snippet.as_str())
                    .size(theme::TEXT_SMALL)
                    .style(theme::muted)
                    .wrapping(Wrapping::WordOrGlyph)
                    .width(Fill),
            ]
            .spacing(theme::SPACING * 0.6)
            .align_y(Center)
            .into()
        }
    };

    button(inner)
        .padding([8, 12])
        .width(Fill)
        .style(row_button(selected))
        .on_press(Message::Activate(index))
        .into()
}

fn distinct_projects(tasks: &[Task]) -> Vec<String> {
    let mut projects: Vec<String> = Vec::new();
    for task in tasks {
        let Some(p) = task.project.as_deref().map(str::trim) else {
            continue;
        };
        if p.is_empty() {
            continue;
        }
        if !projects
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(p))
        {
            projects.push(p.to_string());
        }
    }
    projects.sort_by_key(|p| p.to_ascii_lowercase());
    projects
}

fn task_matches(task: &Task, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let title = task.title.to_ascii_lowercase();
    let project = task
        .project
        .as_deref()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let notes = task.notes.to_ascii_lowercase();
    terms.iter().all(|term| {
        if let Some(tag) = term.strip_prefix('#') {
            if tag.is_empty() {
                !project.is_empty()
            } else {
                project.contains(tag) || title.contains(term) || notes.contains(term)
            }
        } else {
            title.contains(term) || project.contains(term) || notes.contains(term)
        }
    })
}

fn log_matches(log: &LogLine, terms: &[String]) -> bool {
    let text = log.text.to_ascii_lowercase();
    let date_iso = log.date.to_string();
    let date_human = log
        .date
        .strftime("%a %-d %b %Y")
        .to_string()
        .to_ascii_lowercase();
    terms
        .iter()
        .all(|term| text.contains(term) || date_iso.contains(term) || date_human.contains(term))
}

fn make_snippet(text: &str, terms: &[String]) -> String {
    let first_match = text
        .lines()
        .map(str::trim)
        .find(|line| {
            !line.is_empty() && {
                let lower = line.to_ascii_lowercase();
                terms.iter().any(|t| lower.contains(t))
            }
        })
        .or_else(|| text.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("");
    const LIMIT: usize = 90;
    if first_match.chars().count() <= LIMIT {
        first_match.to_string()
    } else {
        let mut s: String = first_match.chars().take(LIMIT - 1).collect();
        s.push('…');
        s
    }
}

fn compare_tasks(a: &Task, b: &Task, today: Date) -> Ordering {
    let bucket = |t: &Task| match t.status {
        TaskStatus::Todo | TaskStatus::Doing => {
            if t.is_overdue(today) {
                0
            } else {
                1
            }
        }
        TaskStatus::Done => 2,
        TaskStatus::Dropped => 3,
    };
    bucket(a)
        .cmp(&bucket(b))
        .then_with(|| match (a.day().or(a.due_on), b.day().or(b.due_on)) {
            (Some(da), Some(db)) => da.cmp(&db),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
        .then(b.priority.cmp(&a.priority))
        .then(a.position.total_cmp(&b.position))
        .then(a.id.cmp(&b.id))
}

fn task_date_label(task: &Task, today: Date) -> Option<String> {
    let format_day = |d: Date| -> String {
        if d == today {
            "Today".into()
        } else if today.tomorrow().ok() == Some(d) {
            "Tomorrow".into()
        } else if today.yesterday().ok() == Some(d) {
            "Yesterday".into()
        } else {
            d.strftime("%a %-d %b").to_string()
        }
    };
    match (task.day(), task.due_on) {
        (Some(p), Some(d)) if p != d => Some(format!("{} · due {}", format_day(p), format_day(d))),
        (Some(p), _) => Some(format_day(p)),
        (None, Some(d)) => Some(format!("due {}", format_day(d))),
        (None, None) => None,
    }
}

fn chip_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        if active {
            button::Style {
                background: Some(theme::accent_color().into()),
                text_color: Color::WHITE,
                border: border::rounded(999),
                ..button::Style::default()
            }
        } else {
            let bg = match status {
                button::Status::Hovered | button::Status::Pressed => theme::tint(theme, 0.12),
                _ => theme::tint(theme, 0.065),
            };
            button::Style {
                background: Some(bg.into()),
                text_color: theme::muted_color(theme),
                border: border::rounded(999),
                ..button::Style::default()
            }
        }
    }
}

fn row_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let p = theme.palette();
        let bg = if selected {
            Some(theme::tint(theme, 0.10).into())
        } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            Some(theme::soft_fill(theme).into())
        } else {
            None
        };
        let border = if selected {
            Border {
                radius: theme::RADIUS.into(),
                width: 1.0,
                color: theme::accent_color().scale_alpha(0.6),
            }
        } else {
            border::rounded(theme::RADIUS)
        };
        button::Style {
            background: bg,
            text_color: p.text,
            border,
            ..button::Style::default()
        }
    }
}

fn divider(theme: &Theme) -> rule::Style {
    rule::Style {
        color: theme::muted_color(theme).scale_alpha(0.25),
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    const TODAY: Date = date(2026, 11, 26);

    fn task(id: TaskId, title: &str, status: TaskStatus) -> Task {
        Task {
            id,
            title: title.into(),
            status,
            planned_for: Some(TODAY),
            position: id as f64,
            ..Task::default()
        }
    }

    fn sample_data() -> SearchData {
        let mut t1 = task(1, "Write launch blog post", TaskStatus::Todo);
        t1.project = Some("Marketing".into());
        t1.notes = "Include benchmarks and screenshots".into();
        t1.priority = 2;

        let mut t2 = task(2, "Fix SQLite migration bug", TaskStatus::Todo);
        t2.project = Some("core".into());
        t2.planned_for = Some(date(2026, 11, 27));
        t2.due_on = Some(date(2026, 11, 28));

        let mut t3 = task(3, "Ship v1.0 release", TaskStatus::Done);
        t3.project = Some("Core".into());

        let mut t4 = task(4, "Old abandoned idea", TaskStatus::Dropped);
        t4.project = Some("Ideas".into());

        let logs = vec![
            LogLine {
                date: date(2026, 11, 25),
                text: "Met with design team about onboarding flow".into(),
            },
            LogLine {
                date: date(2026, 11, 24),
                text: "Reviewed SQLite query plans".into(),
            },
        ];

        SearchData {
            tasks: vec![t1, t2, t3, t4],
            logs,
            today: TODAY,
        }
    }

    #[test]
    fn filters_by_multi_word_query_across_title_notes_and_project() {
        let mut v = SearchView::new();
        v.load(sample_data());

        // Default Open mode hides Done & Dropped tasks.
        assert_eq!(v.results.len(), 2);

        // Multi-word across title + notes (case-insensitive).
        v.update(Message::QueryChanged("LAUNCH screenshots".into()));
        assert_eq!(v.results.len(), 1);
        assert_eq!(
            v.activate_selected(),
            Action::OpenTask { id: 1, day: TODAY }
        );

        // Hash-prefixed project term.
        v.update(Message::QueryChanged("#core".into()));
        assert_eq!(v.results.len(), 1);
        assert_eq!(
            v.activate_selected(),
            Action::OpenTask {
                id: 2,
                day: date(2026, 11, 27)
            }
        );
    }

    #[test]
    fn project_chips_are_sorted_deduplicated_and_toggleable() {
        let mut v = SearchView::new();
        v.load(sample_data());
        // "core" and "Core" deduplicate; sorted case-insensitively.
        assert_eq!(v.projects, vec!["core", "Ideas", "Marketing"]);

        v.update(Message::SelectProject(Some("Marketing".into())));
        assert_eq!(v.results.len(), 1);
        assert_eq!(
            v.activate_selected(),
            Action::OpenTask { id: 1, day: TODAY }
        );

        // Clicking the same chip again toggles it off.
        v.update(Message::SelectProject(Some("Marketing".into())));
        assert_eq!(v.selected_project, None);
        assert_eq!(v.results.len(), 2);
    }

    #[test]
    fn open_vs_all_status_toggle() {
        let mut v = SearchView::new();
        v.load(sample_data());
        assert_eq!(v.results.len(), 2);

        v.update(Message::SetIncludeAll(true));
        assert_eq!(v.results.len(), 4);

        v.update(Message::QueryChanged("abandoned".into()));
        assert_eq!(v.results.len(), 1);
        assert_eq!(
            v.activate_selected(),
            Action::OpenTask { id: 4, day: TODAY }
        );

        v.update(Message::SetIncludeAll(false));
        assert!(v.results.is_empty());
    }

    #[test]
    fn day_notes_match_and_open_day() {
        let mut v = SearchView::new();
        v.load(sample_data());

        v.update(Message::QueryChanged("onboarding".into()));
        assert_eq!(v.results.len(), 1);
        assert_eq!(v.activate_selected(), Action::OpenDay(date(2026, 11, 25)));

        // Query matching both a task and a day note returns task first, then note.
        v.update(Message::QueryChanged("sqlite".into()));
        assert_eq!(v.results.len(), 2);
        assert_eq!(
            v.activate_selected(),
            Action::OpenTask {
                id: 2,
                day: date(2026, 11, 27)
            }
        );
        v.move_selection(1);
        assert_eq!(v.activate_selected(), Action::OpenDay(date(2026, 11, 24)));
    }

    #[test]
    fn keyboard_navigation_clamps_and_reset_clears_state() {
        let mut v = SearchView::new();
        v.load(sample_data());
        assert_eq!(v.selected, 0);

        v.move_selection(-1);
        assert_eq!(v.selected, 0);

        v.move_selection(1);
        assert_eq!(v.selected, 1);

        v.move_selection(10);
        assert_eq!(v.selected, 1);

        assert_eq!(
            v.update(Message::ActivateSelected),
            Action::OpenTask {
                id: 2,
                day: date(2026, 11, 27)
            }
        );

        v.update(Message::QueryChanged("blog".into()));
        v.update(Message::SetIncludeAll(true));
        v.reset();
        assert!(v.query.is_empty());
        assert!(!v.include_all);
        assert_eq!(v.selected_project, None);
        assert_eq!(v.selected, 0);
        assert_eq!(v.results.len(), 2);
        assert_eq!(v.update(Message::Close), Action::Close);
        let _ = v.view();
    }
}
