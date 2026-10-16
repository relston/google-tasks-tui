pub mod atomic_file;
pub mod board;
pub mod cache;
pub mod cli;
pub mod config;
pub mod desktop;
pub mod google;
pub mod keys;
pub mod markdown;
pub mod model;
pub mod paths;
pub mod theme;
pub mod tui;

use std::io;
use std::process::ExitCode;

use clap::Parser;

use cli::Args;
use paths::Paths;

/// No subcommand opens the TUI; any subcommand is a one-shot CLI.
pub fn entry() -> ExitCode {
    let args = Args::parse();
    if args.default_config {
        print!("{}", config::default_toml());
        return ExitCode::SUCCESS;
    }
    let paths = Paths::from_env();
    let result = match args.command {
        None => tui::start(&paths).map_err(|error| (error.to_string(), 1)),
        Some(command) => cli::run(&paths, command, args.format, &mut io::stdout())
            .map_err(|error| (error.to_string(), error.exit_code())),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err((message, code)) => {
            eprintln!("{message}");
            ExitCode::from(code)
        }
    }
}
