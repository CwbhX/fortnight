//! Command-line arguments (clap).

use clap::{Args, Parser, Subcommand};
use fortnight_core::domain::{MAX_PRIORITY, Priority, Protocol, Recurrence, TaskId};
use fortnight_core::export::ExportFormat;
use fortnight_core::parse;
use jiff::civil::Date;
use std::path::PathBuf;

/// Fortnight from the command line: tasks, weekly summary and plan.
#[derive(Debug, Parser)]
#[command(name = "fortnight-cli", version)]
pub struct Cli {
    /// The data folder (default: FORTNIGHT_DATA_DIR, then fortnight-data/
    /// beside the executable).
    #[arg(long, global = true, value_name = "PATH")]
    pub data_dir: Option<PathBuf>,

    /// Pretend today is this date (for tests). Not global: it goes before
    /// the subcommand, since `list --today` is a different flag.
    #[arg(long, hide = true, value_name = "YYYY-MM-DD")]
    pub today: Option<Date>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Add a task. Understands dates (fri, tomorrow, nov 20, +3d), due:<date>,
    /// every:<freq>, #project, ~2h estimates and ! priority.
    Add {
        #[arg(required = true, num_args = 1.., value_name = "WORDS")]
        words: Vec<String>,
        /// Mark the task private (never sent to the LLM).
        #[arg(long)]
        private: bool,
    },
    /// List open tasks: overdue first, then by day, then undated.
    List(ListArgs),
    /// Mark tasks done (or back to todo with --undo).
    Done {
        #[arg(required = true, num_args = 1.., value_name = "ID")]
        ids: Vec<TaskId>,
        /// Set the tasks back to todo.
        #[arg(long)]
        undo: bool,
    },
    /// Delete tasks (soft-delete to dropped, or permanently with --forever).
    Delete {
        #[arg(required = true, num_args = 1.., value_name = "ID")]
        ids: Vec<TaskId>,
        /// Permanently delete the tasks and their history instead of dropping them.
        #[arg(long)]
        forever: bool,
    },
    /// Edit fields on an existing task.
    Edit(EditArgs),
    /// Add a note for today (the Today tab's Notes), or with --task a log
    /// line on that task.
    Log {
        /// Log the line on this task instead.
        #[arg(long, value_name = "ID")]
        task: Option<TaskId>,
        #[arg(required = true, num_args = 1.., value_name = "WORDS")]
        words: Vec<String>,
    },
    /// Write the weekly summary (default: the latest week whose summary is
    /// due).
    Summary(ReportArgs),
    /// Recommend the tasks to do first today.
    Plan(PlanArgs),
    /// Generate whatever reports are due, then exit.
    RunDue(LlmArgs),
    /// Export all tasks, history and reports.
    Export(ExportArgs),
}

#[derive(Debug, Args, Default)]
pub struct ListArgs {
    /// Include done and dropped tasks.
    #[arg(long)]
    pub all: bool,
    /// Only the Today list (tasks planned for today, carried over from
    /// earlier days, or undated) and overdue tasks. With --all, also today's
    /// closed tasks.
    #[arg(long, conflicts_with = "week")]
    pub today: bool,
    /// Only overdue tasks and tasks for this week.
    #[arg(long, conflicts_with = "today")]
    pub week: bool,
    /// Only tasks in this project (case-insensitive).
    #[arg(long, value_name = "PROJECT")]
    pub project: Option<String>,
}

#[derive(Debug, Args, Default)]
pub struct EditArgs {
    /// The task ID to edit.
    #[arg(value_name = "ID")]
    pub id: TaskId,
    /// Replace the task title.
    #[arg(long, value_name = "TEXT")]
    pub title: Option<String>,
    /// Set the project name.
    #[arg(long, value_name = "NAME", conflicts_with = "clear_project")]
    pub project: Option<String>,
    /// Clear the project.
    #[arg(long, conflicts_with = "project")]
    pub clear_project: bool,
    /// Set the deadline (YYYY-MM-DD, today, tomorrow, fri, nov 20, +3d).
    #[arg(long, value_name = "DATE", conflicts_with = "clear_due")]
    pub due: Option<String>,
    /// Clear the deadline.
    #[arg(long, conflicts_with = "due")]
    pub clear_due: bool,
    /// Set the planned day (YYYY-MM-DD, today, tomorrow, fri, nov 20, +3d).
    #[arg(long, value_name = "DATE", conflicts_with = "clear_planned")]
    pub planned: Option<String>,
    /// Clear the planned day.
    #[arg(long, conflicts_with = "planned")]
    pub clear_planned: bool,
    /// Set the effort estimate (e.g. 30m, 2h, 1h30m, 1.5h).
    #[arg(
        long,
        value_name = "EST",
        conflicts_with = "clear_estimate",
        value_parser = parse_estimate_arg
    )]
    pub estimate: Option<u32>,
    /// Clear the effort estimate.
    #[arg(long, conflicts_with = "estimate")]
    pub clear_estimate: bool,
    /// Set priority (0..=3).
    #[arg(long, value_name = "0..=3", value_parser = parse_priority_arg)]
    pub priority: Option<Priority>,
    /// Mark the task private (never sent to the LLM).
    #[arg(long, conflicts_with = "no_private")]
    pub private: bool,
    /// Mark the task not private.
    #[arg(long, conflicts_with = "private")]
    pub no_private: bool,
    /// Set repeat cadence (daily, weekdays, weekly, monthly).
    #[arg(
        long,
        value_name = "CADENCE",
        conflicts_with = "clear_repeat",
        value_parser = parse_recurrence_arg
    )]
    pub repeat: Option<Recurrence>,
    /// Clear the repeat cadence.
    #[arg(long, conflicts_with = "repeat")]
    pub clear_repeat: bool,
}

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// `this`, `last`, or any date in the week (YYYY-MM-DD).
    #[arg(long, value_name = "WEEK")]
    pub week: Option<String>,
    /// Print the deterministic facts as Markdown instead; never calls the
    /// LLM.
    #[arg(long)]
    pub facts: bool,
    #[command(flatten)]
    pub llm: LlmArgs,
}

#[derive(Debug, Args)]
pub struct PlanArgs {
    /// Print the deterministic facts as Markdown instead; never calls the
    /// LLM.
    #[arg(long)]
    pub facts: bool,
    #[command(flatten)]
    pub llm: LlmArgs,
}

/// Flags for commands that call the LLM; they win over the environment and
/// the saved settings.
#[derive(Debug, Args, Default, Clone)]
pub struct LlmArgs {
    /// `openai` or `anthropic`.
    #[arg(long, value_parser = parse_protocol)]
    pub protocol: Option<Protocol>,
    #[arg(long, value_name = "URL")]
    pub base_url: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
    /// The endpoint needs no API key: send none. Overriding the base URL
    /// otherwise assumes it needs one.
    #[arg(long)]
    pub keyless: bool,
    /// Read the key-file password from one line of standard input instead
    /// of prompting.
    #[arg(long)]
    pub password_stdin: bool,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// `md` or `json`.
    #[arg(long, value_parser = parse_format, default_value = "md")]
    pub format: ExportFormat,
    /// Write to this file instead of standard output.
    #[arg(long, value_name = "PATH")]
    pub out: Option<PathBuf>,
}

/// A chat protocol for the main endpoint: `openai` or `anthropic`. The
/// decision protocol only answers recommendations, so it is refused here.
pub fn parse_protocol(s: &str) -> Result<Protocol, String> {
    match Protocol::parse(s) {
        Some(p @ (Protocol::OpenAi | Protocol::Anthropic)) => Ok(p),
        _ => Err(format!("unknown protocol '{s}'. Use openai or anthropic.")),
    }
}

fn parse_format(s: &str) -> Result<ExportFormat, String> {
    ExportFormat::parse(s).ok_or_else(|| format!("unknown format '{s}'. Use md or json."))
}

fn parse_estimate_arg(s: &str) -> Result<u32, String> {
    parse::parse_estimate(s)
        .ok_or_else(|| format!("invalid estimate '{s}'. Use e.g. 30m, 2h, 1h30m or 1.5h."))
}

fn parse_priority_arg(s: &str) -> Result<Priority, String> {
    let p: Priority = s
        .trim()
        .parse()
        .map_err(|_| format!("invalid priority '{s}'. Use 0..={MAX_PRIORITY}."))?;
    if p <= MAX_PRIORITY {
        Ok(p)
    } else {
        Err(format!("invalid priority '{s}'. Use 0..={MAX_PRIORITY}."))
    }
}

fn parse_recurrence_arg(s: &str) -> Result<Recurrence, String> {
    Recurrence::parse(s).ok_or_else(|| {
        format!("unknown repeat cadence '{s}'. Use daily, weekdays, weekly or monthly.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("fortnight-cli").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn add_takes_the_rest_as_words() {
        let cli = parse(&["add", "write", "report", "fri", "#work", "~2h", "!", "+3d"]);
        match cli.command {
            Command::Add { words, private } => {
                assert_eq!(words.len(), 7);
                assert!(!private);
            }
            c => panic!("{c:?}"),
        }

        let cli = parse(&["add", "--private", "secret", "plan"]);
        match cli.command {
            Command::Add { words, private } => {
                assert_eq!(words, vec!["secret", "plan"]);
                assert!(private);
            }
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn data_dir_is_global_and_today_is_not_the_list_flag() {
        let cli = parse(&[
            "--today",
            "2026-11-16",
            "list",
            "--data-dir",
            "/x",
            "--today",
            "--project",
            "Work",
        ]);
        assert_eq!(cli.data_dir.as_deref(), Some(std::path::Path::new("/x")));
        assert_eq!(cli.today, Some(jiff::civil::date(2026, 11, 16)));
        match cli.command {
            Command::List(ListArgs {
                today: true,
                project,
                ..
            }) => assert_eq!(project.as_deref(), Some("Work")),
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn delete_and_edit_flags_parse() {
        let cli = parse(&["delete", "1", "2"]);
        match cli.command {
            Command::Delete { ids, forever } => {
                assert_eq!(ids, vec![1, 2]);
                assert!(!forever);
            }
            c => panic!("{c:?}"),
        }

        let cli = parse(&["delete", "--forever", "3"]);
        match cli.command {
            Command::Delete { ids, forever } => {
                assert_eq!(ids, vec![3]);
                assert!(forever);
            }
            c => panic!("{c:?}"),
        }

        let cli = parse(&[
            "edit",
            "5",
            "--title",
            "new title",
            "--project",
            "ops",
            "--due",
            "2026-11-25",
            "--planned",
            "+2d",
            "--estimate",
            "1h30m",
            "--priority",
            "3",
            "--private",
            "--repeat",
            "weekdays",
        ]);
        match cli.command {
            Command::Edit(a) => {
                assert_eq!(a.id, 5);
                assert_eq!(a.title.as_deref(), Some("new title"));
                assert_eq!(a.project.as_deref(), Some("ops"));
                assert!(!a.clear_project);
                assert_eq!(a.due.as_deref(), Some("2026-11-25"));
                assert!(!a.clear_due);
                assert_eq!(a.planned.as_deref(), Some("+2d"));
                assert!(!a.clear_planned);
                assert_eq!(a.estimate, Some(90));
                assert!(!a.clear_estimate);
                assert_eq!(a.priority, Some(3));
                assert!(a.private);
                assert!(!a.no_private);
                assert_eq!(a.repeat, Some(Recurrence::Weekdays));
                assert!(!a.clear_repeat);
            }
            c => panic!("{c:?}"),
        }

        let cli = parse(&[
            "edit",
            "5",
            "--clear-project",
            "--clear-due",
            "--clear-planned",
            "--clear-estimate",
            "--no-private",
            "--clear-repeat",
        ]);
        match cli.command {
            Command::Edit(a) => {
                assert!(a.clear_project);
                assert!(a.clear_due);
                assert!(a.clear_planned);
                assert!(a.clear_estimate);
                assert!(a.no_private);
                assert!(a.clear_repeat);
            }
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn llm_flags_parse() {
        let cli = parse(&[
            "summary",
            "--week",
            "last",
            "--protocol",
            "Anthropic",
            "--model",
            "m",
            "--password-stdin",
            "--keyless",
        ]);
        match cli.command {
            Command::Summary(a) => {
                assert_eq!(a.week.as_deref(), Some("last"));
                assert_eq!(a.llm.protocol, Some(Protocol::Anthropic));
                assert_eq!(a.llm.model.as_deref(), Some("m"));
                assert!(a.llm.password_stdin);
                assert!(a.llm.keyless);
                assert!(!a.facts);
            }
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn bad_values_are_rejected() {
        assert!(Cli::try_parse_from(["f", "export", "--format", "xml"]).is_err());
        assert!(Cli::try_parse_from(["f", "plan", "--protocol", "grpc"]).is_err());
        assert!(Cli::try_parse_from(["f", "summary", "--protocol", "decision"]).is_err());
        assert!(Cli::try_parse_from(["f", "plan", "--protocol", "ollama_decision"]).is_err());
        // Recommendations are about today: `plan` has no --week.
        assert!(Cli::try_parse_from(["f", "plan", "--week", "last"]).is_err());
        assert!(Cli::try_parse_from(["f", "done"]).is_err());
        assert!(Cli::try_parse_from(["f", "delete"]).is_err());
        assert!(Cli::try_parse_from(["f", "edit"]).is_err());
        assert!(Cli::try_parse_from(["f", "add"]).is_err());
        assert!(Cli::try_parse_from(["f", "list", "--today", "--week"]).is_err());
        // Conflicting edit flags.
        assert!(
            Cli::try_parse_from(["f", "edit", "1", "--project", "a", "--clear-project"]).is_err()
        );
        assert!(Cli::try_parse_from(["f", "edit", "1", "--due", "today", "--clear-due"]).is_err());
        assert!(
            Cli::try_parse_from(["f", "edit", "1", "--planned", "today", "--clear-planned"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from(["f", "edit", "1", "--estimate", "1h", "--clear-estimate"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["f", "edit", "1", "--private", "--no-private"]).is_err());
        assert!(
            Cli::try_parse_from(["f", "edit", "1", "--repeat", "daily", "--clear-repeat"]).is_err()
        );
        // Invalid edit values.
        assert!(Cli::try_parse_from(["f", "edit", "1", "--priority", "4"]).is_err());
        assert!(Cli::try_parse_from(["f", "edit", "1", "--estimate", "0m"]).is_err());
        assert!(Cli::try_parse_from(["f", "edit", "1", "--estimate", "nope"]).is_err());
        assert!(Cli::try_parse_from(["f", "edit", "1", "--repeat", "yearly"]).is_err());
    }

    #[test]
    fn trailing_flags_work_on_add_and_log() {
        let cli = parse(&["add", "buy", "milk", "--private", "--data-dir", "/tmp/d"]);
        assert_eq!(
            cli.data_dir.as_deref(),
            Some(std::path::Path::new("/tmp/d"))
        );
        match cli.command {
            Command::Add { words, private } => {
                assert_eq!(words, vec!["buy", "milk"]);
                assert!(private);
            }
            c => panic!("{c:?}"),
        }

        let cli = parse(&["log", "did", "thing", "--task", "3", "--data-dir", "/tmp/d"]);
        assert_eq!(
            cli.data_dir.as_deref(),
            Some(std::path::Path::new("/tmp/d"))
        );
        match cli.command {
            Command::Log { task, words } => {
                assert_eq!(task, Some(3));
                assert_eq!(words, vec!["did", "thing"]);
            }
            c => panic!("{c:?}"),
        }
    }
}
