//! Entry point of the Claude Code adapter: one bounded provider turn recorded as a managed run.
//! Standard output carries one JSON summary line; every diagnostic goes to standard error.

use dpm_claude::{CommandError, Options, OptionsError, ParsedOptions, USAGE, execute_blocking};
use serde_json::json;
use std::{
    io::{self, Write},
    process::ExitCode,
    sync::atomic::AtomicBool,
};
use tracing_subscriber::filter::LevelFilter;

/// Exit codes a caller can tell apart without parsing text.
mod exit {
    /// The provider reported a finished turn and the run was recorded.
    pub const COMPLETED: u8 = 0;
    /// The run ended without a finished turn and was recorded.
    pub const ENDED: u8 = 1;
    /// The command line, the workspace, the task or the provider could not be used.
    pub const START: u8 = 2;
    /// The end of the run could not be recorded.
    pub const UNRECORDED: u8 = 3;
}

fn say(line: &str) {
    let mut out = io::stdout().lock();
    if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
        tracing::error!("the summary could not be written");
    }
}

fn fail(error: &(dyn std::error::Error + 'static)) -> ExitCode {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        text.push_str(": ");
        text.push_str(&next.to_string());
        cause = next.source();
    }
    tracing::error!("{text}");
    ExitCode::from(exit::START)
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_max_level(
            if arguments.iter().any(|argument| argument == "--verbose") {
                LevelFilter::DEBUG
            } else {
                LevelFilter::WARN
            },
        )
        .init();
    let options = match Options::parse(arguments) {
        Ok(ParsedOptions::Run(options)) => options,
        Ok(ParsedOptions::Help) => {
            return if io::stdout().write_all(USAGE.as_bytes()).is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(exit::START)
            };
        }
        Err(error @ (OptionsError::Usage(_) | OptionsError::ApprovingMode(_))) => {
            return fail(&error);
        }
    };
    let cancel = AtomicBool::new(false);
    match execute_blocking(&options, &cancel) {
        Ok(outcome) => {
            let report = &outcome.report;
            say(&json!({
                "run": outcome.run.to_string(),
                "recovered": outcome.recovered.map(|run| run.to_string()),
                "state": report.terminal.state.word(),
                "evidence": report.terminal.evidence.word(),
                "detail": report.terminal.detail,
                "terminal_recorded": report.terminal_recorded,
                "records": report.tally.recorded,
                "repeated": report.tally.repeated,
                "conflicts": report.tally.conflicts,
                "malformed": report.tally.malformed,
                "provider_killed": report.stopped.killed,
                "provider_reaped": report.stopped.reaped,
                "pending_high_water": report.pending_high_water,
                "stderr_bytes": report.stderr_bytes,
                "replay_window_complete": report.window_complete,
                "recording_failure": report.sink_failure.as_ref().map(dpm_claude::SinkError::chain),
            })
            .to_string());
            if !report.terminal_recorded {
                ExitCode::from(exit::UNRECORDED)
            } else if report.terminal.state == dpm_model::RunState::Completed {
                ExitCode::from(exit::COMPLETED)
            } else {
                ExitCode::from(exit::ENDED)
            }
        }
        Err(
            error @ (CommandError::Workspace(_)
            | CommandError::Prompt(_)
            | CommandError::PromptFile { .. }
            | CommandError::Directory(_)
            | CommandError::Task { .. }
            | CommandError::Recover { .. }
            | CommandError::Host(_)
            | CommandError::Provider(_)
            | CommandError::Begin { .. }),
        ) => fail(&error),
    }
}
