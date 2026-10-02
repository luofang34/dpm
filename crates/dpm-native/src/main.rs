//! Entry point of the persistent local helper: standard input and output carry the protocol and
//! nothing else; every diagnostic goes to standard error through tracing.

use dpm_native::{
    Ending, MIN_FRAME_BYTES, Options, Parsed, StartError, USAGE, serve_blocking, start_blocking,
};
use std::{
    io::{self, BufReader, Write},
    process::ExitCode,
};
use tracing_subscriber::filter::LevelFilter;

/// Exit codes a host can tell apart without parsing text.
mod exit {
    /// The input closed between frames.
    pub const CLOSED: u8 = 0;
    /// Reading or writing failed.
    pub const IO: u8 = 1;
    /// The command line or the workspace could not be used; nothing was served.
    pub const START: u8 = 2;
    /// The input closed inside a frame.
    pub const TRUNCATED: u8 = 3;
}

/// Refuse to serve: log it, and tell the host in one protocol frame so it need not read text.
fn refuse(error: &StartError, limit: usize) -> ExitCode {
    tracing::error!(code = error.code(), %error, "not serving");
    let mut output = io::stdout().lock();
    if writeln!(output, "{}", error.refusal_line(limit))
        .and_then(|()| output.flush())
        .is_err()
    {
        tracing::error!("the refusal could not be written to the host");
    }
    ExitCode::from(exit::START)
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    // Logging starts before anything can fail, so every failure below has somewhere to go.
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
        Ok(Parsed::Run(options)) => options,
        Ok(Parsed::Help) => {
            // No protocol session was started, so the usage text may go to standard output.
            return ExitCode::from(if io::stdout().write_all(USAGE.as_bytes()).is_ok() {
                exit::CLOSED
            } else {
                exit::IO
            });
        }
        // Bounds that could not be read are the smallest, which every refusal fits.
        Err(error) => return refuse(&StartError::Options(error), MIN_FRAME_BYTES),
    };
    let app = match start_blocking(&options) {
        Ok(app) => app,
        Err(error) => return refuse(&error, options.limits.max_response_bytes()),
    };
    match serve_blocking(
        app,
        BufReader::new(io::stdin()),
        io::stdout(),
        options.limits,
    ) {
        Ok(Ending::Closed) => ExitCode::from(exit::CLOSED),
        Ok(Ending::Truncated) => ExitCode::from(exit::TRUNCATED),
        Err(error) => {
            tracing::error!(%error, "serving stopped");
            ExitCode::from(exit::IO)
        }
    }
}
