//! Human and JSON adapters over validated DPM command/query operations.

mod app;
mod args;
mod bootstrap;
mod clock;
mod console;
mod error;
mod interchange;
mod output;
mod ownership;
mod recovery;
mod tracking;
mod usage;

use clap::Parser;
use std::process::ExitCode;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .without_time()
        .with_target(false)
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .init();
    let cli = match args::Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            return usage::report_blocking(&error, usage::json_requested(std::env::args_os()));
        }
    };
    let json = cli.json;
    match app::run_blocking(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if json {
                if let Err(output_error) = output::json_blocking(&error.envelope()) {
                    tracing::error!(%output_error, "could not write machine error");
                }
            } else {
                tracing::error!("{error}");
            }
            ExitCode::FAILURE
        }
    }
}
