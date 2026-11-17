//! The glue between the command line and the commands: data folder, store,
//! lock warning, runtime, exit codes.

use crate::args::{Cli, Command};
use crate::commands;
use crate::error::{CliError, CliResult};
use crate::llm_setup;
use fortnight_core::datadir;
use fortnight_core::domain::ReportKind;
use fortnight_core::sqlite::SqliteStore;
use fortnight_core::store::Store;
use jiff::tz::TimeZone;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A warning (for stderr) if the data folder's lock is held by another
/// machine. Our own machine's lock is the GUI, which is fine: the CLI does
/// not take the lock and SQLite copes with two processes. The same Mac
/// with and without `.local` counts as this machine.
pub fn lock_warning(dir: &Path, this_host: &str) -> Option<String> {
    let lock = datadir::read_lock(dir)?;
    datadir::held_elsewhere(&lock, this_host).then(|| {
        // The lock file is untrusted (it may arrive through a sync service):
        // keep control characters out of the terminal.
        let host: String = lock.host.chars().filter(|c| !c.is_control()).collect();
        format!(
            "warning: the data folder is also open on {host}, since {}. Changes may conflict.",
            lock.since
        )
    })
}

/// Resolves the data folder and opens the store. Only `create` (the `add`
/// command) may create the folder and the database; otherwise a missing
/// database is an error, so a mistyped `--data-dir` doesn't quietly make an
/// empty one. Warns on stderr if another host holds the lock.
pub fn open_store(flag: Option<PathBuf>, create: bool) -> CliResult<(SqliteStore, PathBuf)> {
    let dir = datadir::resolve(flag).map_err(|e| CliError::failure(e.to_string()))?;
    if !create && !datadir::db_path(&dir).exists() {
        return Err(CliError::failure(format!(
            "no Fortnight data in {}. Check --data-dir or FORTNIGHT_DATA_DIR \
             (`fortnight-cli add` creates a new data folder).",
            dir.display()
        )));
    }
    datadir::ensure(&dir).map_err(|e| CliError::failure(e.to_string()))?;
    if let Some(warning) = lock_warning(&dir, &datadir::host_name()) {
        eprintln!("{warning}");
    }
    let store = SqliteStore::open(&datadir::db_path(&dir))?;
    Ok((store, dir))
}

/// Standard output that goes quiet once its reader has gone
/// (`fortnight-cli export | head -1`): later writes are dropped instead of
/// failing, so a command still finishes its work and reports its real
/// result. Any other write error is passed on.
struct UntilClosed<W: Write> {
    inner: W,
    closed: bool,
}

impl<W: Write> UntilClosed<W> {
    fn quiet<T>(&mut self, result: std::io::Result<T>, done: T) -> std::io::Result<T> {
        match result {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                self.closed = true;
                Ok(done)
            }
            other => other,
        }
    }
}

impl<W: Write> Write for UntilClosed<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.closed {
            return Ok(buf.len());
        }
        let result = self.inner.write(buf);
        self.quiet(result, buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.closed {
            return Ok(());
        }
        let result = self.inner.flush();
        self.quiet(result, ())
    }
}

/// Runs one command and returns the process exit code.
pub fn main_with(cli: Cli) -> i32 {
    let mut out = UntilClosed {
        inner: std::io::stdout().lock(),
        closed: false,
    };
    match execute(cli, &mut out) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("fortnight-cli: {}", e.message);
            e.code
        }
    }
}

fn execute(cli: Cli, out: &mut dyn Write) -> CliResult<()> {
    let tz = TimeZone::system();
    let now = commands::now_for(cli.today, tz)?;
    let today = now.date();
    let create = matches!(cli.command, Command::Add { .. });
    let (store, dir) = open_store(cli.data_dir, create)?;
    let store: &dyn Store = &store;
    match cli.command {
        Command::Add { words } => commands::add(store, &words, today, out),
        Command::List(opts) => commands::list(store, &opts, today, out),
        Command::Done { ids, undo } => commands::done(store, &ids, undo, out),
        Command::Log { task, words } => commands::log(store, task, &words, out),
        Command::Export(a) => commands::export(store, a.format, &now, a.out.as_deref(), out),
        Command::Summary(a) => block_on(async {
            let saved = store.load_settings()?;
            let make = |uses_decision| {
                llm_setup::connect_from_settings(
                    &saved.llm,
                    &saved.decision_llm,
                    &a.llm,
                    &dir,
                    uses_decision,
                )
            };
            commands::report_command(
                store,
                ReportKind::Summary,
                a.week.as_deref(),
                a.facts,
                &now,
                &make,
                out,
            )
            .await
        }),
        Command::Plan(a) => block_on(async {
            let saved = store.load_settings()?;
            let make = |uses_decision| {
                llm_setup::connect_from_settings(
                    &saved.llm,
                    &saved.decision_llm,
                    &a.llm,
                    &dir,
                    uses_decision,
                )
            };
            commands::report_command(store, ReportKind::Plan, None, a.facts, &now, &make, out).await
        }),
        Command::RunDue(llm) => block_on(async {
            let saved = store.load_settings()?;
            let make = |uses_decision| {
                llm_setup::connect_from_settings(
                    &saved.llm,
                    &saved.decision_llm,
                    &llm,
                    &dir,
                    uses_decision,
                )
            };
            commands::run_due(store, &now, &make, out).await
        }),
    }
}

fn block_on<F: std::future::Future<Output = CliResult<()>>>(fut: F) -> CliResult<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::failure(e.to_string()))?
        .block_on(fut)
}
