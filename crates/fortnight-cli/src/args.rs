//! Command-line arguments (clap).

use clap::{Args, Parser, Subcommand};
use fortnight_core::domain::{Protocol, TaskId};
use fortnight_core::export::ExportFormat;
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
    /// Add a task. Understands dates (fri, tomorrow, nov 20, +3d), #project,
    /// ~2h estimates and ! priority.
    Add {
        #[arg(required = true, num_args = 1.., value_name = "WORDS")]
        words: Vec<String>,
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
            Command::Add { words } => assert_eq!(words.len(), 7),
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
        ]);
        assert_eq!(cli.data_dir.as_deref(), Some(std::path::Path::new("/x")));
        assert_eq!(cli.today, Some(jiff::civil::date(2026, 11, 16)));
        assert!(matches!(
            cli.command,
            Command::List(ListArgs { today: true, .. })
        ));
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
        assert!(Cli::try_parse_from(["f", "add"]).is_err());
        assert!(Cli::try_parse_from(["f", "list", "--today", "--week"]).is_err());
    }

    #[test]
    fn trailing_flags_work_on_add_and_log() {
        let cli = parse(&["add", "buy", "milk", "--data-dir", "/tmp/d"]);
        assert_eq!(
            cli.data_dir.as_deref(),
            Some(std::path::Path::new("/tmp/d"))
        );
        match cli.command {
            Command::Add { words } => assert_eq!(words, vec!["buy", "milk"]),
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
