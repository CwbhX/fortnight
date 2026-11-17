//! `fortnight-cli`: the headless shell over the core (PLAN.md section 8).
//! Owner: agent N (round 2). STUB.

use clap::Parser;

/// Fortnight from the command line.
#[derive(Debug, Parser)]
#[command(name = "fortnight-cli", version)]
struct Cli {
    /// The data folder (default: FORTNIGHT_DATA_DIR, then fortnight-data/
    /// beside the executable).
    #[arg(long, global = true)]
    data_dir: Option<std::path::PathBuf>,
}

fn main() {
    let _cli = Cli::parse();
    eprintln!("fortnight-cli: not implemented yet");
    std::process::exit(2);
}
