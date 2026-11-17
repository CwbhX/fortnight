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
/// not take the lock and SQLite copes with two processes.
pub fn lock_warning(dir: &Path, this_host: &str) -> Option<String> {
    let lock = datadir::read_lock(dir)?;
    (lock.host != this_host).then(|| {
        // The lock file is untrusted (it may arrive through a sync service):
        // keep control characters out of the terminal.
        let host: String = lock.host.chars().filter(|c| !c.is_control()).collect();
        format!(
            "warning: the data folder is in use on {host} (since {}); changes may conflict",
            lock.since
        )
    })
}

/// Resolves and prepares the data folder and opens the store. Warns on
/// stderr if another host holds the lock.
pub fn open_store(flag: Option<PathBuf>) -> CliResult<(SqliteStore, PathBuf)> {
    let dir = datadir::resolve(flag).map_err(|e| CliError::failure(e.to_string()))?;
    datadir::ensure(&dir).map_err(|e| CliError::failure(e.to_string()))?;
    if let Some(warning) = lock_warning(&dir, &datadir::host_name()) {
        eprintln!("{warning}");
    }
    let store = SqliteStore::open(&datadir::db_path(&dir))?;
    Ok((store, dir))
}

/// Runs one command and returns the process exit code.
pub fn main_with(cli: Cli) -> i32 {
    match execute(cli, &mut std::io::stdout().lock()) {
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
    let (store, dir) = open_store(cli.data_dir)?;
    let store: &dyn Store = &store;
    match cli.command {
        Command::Add { words } => commands::add(store, &words, today, out),
        Command::List(opts) => commands::list(store, &opts, today, out),
        Command::Done { ids, undo } => commands::done(store, &ids, undo, out),
        Command::Log { task, words } => commands::log(store, task, &words, out),
        Command::Export(a) => commands::export(store, a.format, today, a.out.as_deref(), out),
        Command::Summary(a) => block_on(async {
            let saved = store.load_settings()?.llm;
            let make = || llm_setup::connect_from_settings(&saved, &a.llm, &dir);
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
            let saved = store.load_settings()?.llm;
            let make = || llm_setup::connect_from_settings(&saved, &a.llm, &dir);
            commands::report_command(
                store,
                ReportKind::Plan,
                a.week.as_deref(),
                a.facts,
                &now,
                &make,
                out,
            )
            .await
        }),
        Command::RunDue(llm) => block_on(async {
            let saved = store.load_settings()?.llm;
            let make = || llm_setup::connect_from_settings(&saved, &llm, &dir);
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
