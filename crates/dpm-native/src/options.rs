//! Source selection and frame bounds, parsed from the helper's command line.
//!
//! The source is selected exactly as the CLI selects it: an explicit `--project` or `--database`
//! overrides discovery, and with neither the nearest project locator at or above the working
//! directory wins. Nothing here initializes a workspace.

use crate::limits::{LimitError, Limits};
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use thiserror::Error;

/// What the helper prints for `--help`.
pub const USAGE: &str = "\
Usage: dpm-native [--project DIR | --database PATH] [--clock RFC3339]
                  [--max-request-bytes N] [--max-response-bytes N] [--verbose]

Serves the native DPM contract as newline-framed JSON on standard input and output.
Without --project or --database the nearest project locator at or above the working directory
is used. Nothing is initialized: a missing workspace is an error.
";

/// A command line the helper cannot run.
#[derive(Debug, Error)]
pub enum OptionsError {
    /// A flag the helper does not have, or one that lacks its value.
    #[error("{0}")]
    Usage(String),
    /// `--project` and `--database` select the source and cannot both be given.
    #[error("--project and --database are mutually exclusive")]
    ConflictingSource,
    /// `--clock` is not an RFC 3339 instant.
    #[error("--clock {value:?} is not an RFC 3339 instant: {source}")]
    Clock {
        /// What was given.
        value: String,
        /// Why it did not parse.
        #[source]
        source: chrono::ParseError,
    },
    /// A frame bound is not a whole number of bytes.
    #[error("{flag} needs a whole number of bytes, not {value:?}")]
    Bound {
        /// The flag that carried it.
        flag: &'static str,
        /// What was given.
        value: String,
    },
    /// A frame bound is outside what the helper can honour.
    #[error("{flag}: {source}")]
    Limit {
        /// The flag that carried it.
        flag: &'static str,
        /// What is wrong with the value.
        #[source]
        source: LimitError,
    },
}

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// Serve with these options.
    Run(Options),
    /// Print the usage and exit.
    Help,
}

/// How to open the workspace and how to bound the frames.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    /// Project directory containing `.dpm/project.toml`.
    pub project: Option<PathBuf>,
    /// Explicit store path, overriding discovery.
    pub database: Option<PathBuf>,
    /// Pins the query clock for the whole session, as `dpm --clock` does; a client that needs
    /// reproducible time-dependent answers starts the helper this way.
    pub clock: Option<DateTime<Utc>>,
    /// Frame bounds.
    pub limits: Limits,
    /// Whether to log request-level detail to standard error.
    pub verbose: bool,
}

fn bytes(flag: &'static str, value: String) -> Result<usize, OptionsError> {
    value
        .parse::<usize>()
        .map_err(|_| OptionsError::Bound { flag, value })
}

impl Options {
    /// Parse the arguments after the program name.
    pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Parsed, OptionsError> {
        let mut options = Self::default();
        let (mut request, mut response) = (None, None);
        let mut arguments = arguments.into_iter();
        while let Some(flag) = arguments.next() {
            let mut value = |name: &str| {
                arguments
                    .next()
                    .ok_or_else(|| OptionsError::Usage(format!("{name} needs a value")))
            };
            match flag.as_str() {
                "--help" | "-h" => return Ok(Parsed::Help),
                "--verbose" => options.verbose = true,
                "--project" => options.project = Some(value("--project")?.into()),
                "--database" => options.database = Some(value("--database")?.into()),
                "--clock" => {
                    let text = value("--clock")?;
                    options.clock = Some(text.parse().map_err(|source| OptionsError::Clock {
                        value: text.clone(),
                        source,
                    })?);
                }
                "--max-request-bytes" => {
                    request = Some(bytes("--max-request-bytes", value(&flag)?)?);
                }
                "--max-response-bytes" => {
                    response = Some(bytes("--max-response-bytes", value(&flag)?)?);
                }
                other => return Err(OptionsError::Usage(format!("unknown argument {other}"))),
            }
        }
        if options.project.is_some() && options.database.is_some() {
            return Err(OptionsError::ConflictingSource);
        }
        let defaults = Limits::default();
        options.limits = Limits::new(
            request.unwrap_or(defaults.max_request_bytes()),
            response.unwrap_or(defaults.max_response_bytes()),
        )
        .map_err(|source| OptionsError::Limit {
            flag: if source.name == "the request bound" {
                "--max-request-bytes"
            } else {
                "--max-response-bytes"
            },
            source,
        })?;
        Ok(Parsed::Run(options))
    }
}

#[cfg(test)]
mod tests;
