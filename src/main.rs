//! Human and JSON adapters over validated DPM command/query operations.

mod app;
mod args;
mod error;
mod interchange;
mod output;
mod recovery;
mod tracking;

use clap::Parser;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = args::Cli::parse();
    let json = cli.json;
    tracing_subscriber::fmt()
        .without_time()
        .with_target(false)
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .init();
    match app::run_blocking(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if json {
                let mut body =
                    serde_json::json!({"code": error.code(), "message": error.to_string()});
                if let Some(details) = error.details() {
                    body["details"] = details;
                }
                if let Err(output_error) =
                    output::json_blocking(&serde_json::json!({ "error": body }))
                {
                    tracing::error!(%output_error, "could not write machine error");
                }
            } else {
                tracing::error!("{error}");
            }
            ExitCode::FAILURE
        }
    }
}
