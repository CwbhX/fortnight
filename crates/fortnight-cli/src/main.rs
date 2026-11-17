//! `fortnight-cli`: the headless shell over the core (PLAN.md section 8).

use clap::Parser;

fn main() {
    let cli = fortnight_cli::args::Cli::parse();
    std::process::exit(fortnight_cli::run::main_with(cli));
}
