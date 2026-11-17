//! Quick-add parser. Owner: agent B.
//!
//! `Write Q4 report fri #work` → title "Write Q4 report", due the coming
//! Friday, project "work". Date tokens: `today`, `tomorrow` (`tmr`),
//! weekday names and 3-letter abbreviations (`fri` = the next Friday,
//! never today), `nov 20` / `20 nov` (this year, or next year if already
//! past), ISO `2026-11-20`, and `+3d` / `+2w`. `#project` sets the project.
//! The last date token and the last `#project` win; everything else,
//! in order, is the title.

use jiff::civil::Date;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuickAdd {
    pub title: String,
    pub project: Option<String>,
    pub due_on: Option<Date>,
}

/// Parses a quick-add line relative to `today`.
pub fn parse_quick_add(_input: &str, _today: Date) -> QuickAdd {
    todo!("agent B")
}

/// Parses a whole string as one date (`fri`, `tomorrow`, `nov 20`, `+3d`,
/// `2026-11-20`, ...). Used by the task edit form. `None` if not a date.
pub fn parse_date(_input: &str, _today: Date) -> Option<Date> {
    todo!("agent B")
}
