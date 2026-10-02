//! A test-only stdio host for the native boundary: one request line in, one response line out.
//!
//! A persistent local helper is one way a native client can reach the shared application. This
//! example is that helper reduced to its host side: it opens one source, then answers every line
//! on standard input with [`Application::native_json_blocking`] and nothing else, so the bytes a
//! client sees are exactly what the boundary produces. It keeps no state per client.
//!
//! The one affordance beyond the protocol is for tests: a line `!clock <RFC 3339 time>` pins the
//! query clock (and `!clock system` releases it), so a proof can evaluate time-dependent views at
//! exact instants without waiting. It is not part of the contract and a production host would not
//! offer it.
//!
//! Usage: `native_exchange (--db PATH | --project DIR | --preview PLAN.json) [--clock TIME]`

use chrono::{DateTime, Utc};
use dpm_app::{AppError, Application, QueryClock, open_workspace_blocking};
use dpm_model::Plan;
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
};
use thiserror::Error;

/// Why the host stopped.
#[derive(Debug, Error)]
enum HostError {
    #[error(
        "usage: native_exchange (--db PATH | --project DIR | --preview PLAN.json) [--clock TIME]"
    )]
    Usage,
    #[error("{0:?} is not an RFC 3339 time")]
    Clock(String, #[source] chrono::ParseError),
    #[error("reading the preview plan {path}: {source}")]
    Plan {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the preview plan is not a plan: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    App(#[from] AppError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// The source to open and the clock to start with.
struct Options {
    source: Source,
    clock: QueryClock,
}

enum Source {
    Database(PathBuf),
    Project(PathBuf),
    Preview(PathBuf),
}

fn parse_clock(text: &str) -> Result<QueryClock, HostError> {
    if text == "system" {
        return Ok(QueryClock::System);
    }
    let at: DateTime<Utc> = text
        .parse()
        .map_err(|error| HostError::Clock(text.to_string(), error))?;
    Ok(QueryClock::Fixed(at))
}

fn parse_options(mut arguments: impl Iterator<Item = String>) -> Result<Options, HostError> {
    let mut source = None;
    let mut clock = QueryClock::System;
    while let Some(flag) = arguments.next() {
        let value = arguments.next().ok_or(HostError::Usage)?;
        match flag.as_str() {
            "--db" => source = Some(Source::Database(value.into())),
            "--project" => source = Some(Source::Project(value.into())),
            "--preview" => source = Some(Source::Preview(value.into())),
            "--clock" => clock = parse_clock(&value)?,
            _ => return Err(HostError::Usage),
        }
    }
    Ok(Options {
        source: source.ok_or(HostError::Usage)?,
        clock,
    })
}

fn open_blocking(source: &Source) -> Result<Application, HostError> {
    let here = std::env::current_dir()?;
    Ok(match source {
        Source::Database(path) => open_workspace_blocking(&here, None, Some(path))?,
        Source::Project(path) => open_workspace_blocking(&here, Some(path), None)?,
        Source::Preview(path) => {
            let text = std::fs::read_to_string(path).map_err(|source| HostError::Plan {
                path: path.clone(),
                source,
            })?;
            Application::preview(serde_json::from_str::<Plan>(&text)?)?
        }
    })
}

fn main() -> Result<(), HostError> {
    let options = parse_options(std::env::args().skip(1))?;
    let mut app = open_blocking(&options.source)?;
    app.set_query_clock(options.clock);
    let mut output = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let answer = match line.strip_prefix("!clock ") {
            Some(text) => {
                let clock = parse_clock(text.trim())?;
                app.set_query_clock(clock);
                r#"{"ok":true}"#.to_string()
            }
            None => app.native_json_blocking(line),
        };
        writeln!(output, "{answer}")?;
        output.flush()?;
    }
    Ok(())
}
