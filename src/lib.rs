pub mod api;
pub mod auth;
pub mod board;
pub mod cache;
pub mod config;
pub mod cli;
pub mod theme;
pub mod tui;

use std::io;
use std::process;

use clap::Parser;

use api::Client;
use cli::Args;

/// No subcommand opens the TUI; any subcommand is a one-shot CLI.
pub fn entry() -> auth::Result<()> {
    let args = Args::parse();
    match args.command {
        None => tui::start(),
        Some(command) => {
            let auth = match auth::Auth::cached() {
                Some(a) => a,
                None => auth::Auth::new()?,
            };
            match cli::execute_to(
                &mut Client::new(auth),
                command,
                args.format,
                &mut io::stdout(),
            ) {
                Ok(()) => Ok(()),
                Err(e) => {
                    eprintln!("{e}");
                    process::exit(e.exit_code());
                }
            }
        }
    }
}
