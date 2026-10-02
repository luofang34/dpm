//! Opening the workspace before any request is read, and refusing it in a form a host can read.
//!
//! The source is selected exactly as the CLI selects it, through the shared application boundary,
//! and a source that is not a workspace is refused with the application's own stable code. A host
//! cannot parse diagnostic text, so a refusal is also written as one protocol frame with an empty
//! identifier, carrying that code and its details, before the helper exits.

use crate::{
    options::{Options, OptionsError},
    rejection::{codes, rejection},
};
use dpm_app::{AppError, Application, QueryClock, open_workspace_blocking};
use serde_json::Value;
use std::io;
use thiserror::Error;

/// Why the helper could not start serving.
#[derive(Debug, Error)]
pub enum StartError {
    /// The command line cannot be run.
    #[error(transparent)]
    Options(#[from] OptionsError),
    /// The working directory, which discovery starts from, is unavailable.
    #[error("the working directory is unavailable: {0}")]
    WorkingDirectory(#[source] io::Error),
    /// The selected source is not a usable workspace.
    #[error("{0}")]
    Workspace(#[source] Box<AppError>),
}

impl StartError {
    /// The stable code of this refusal: the application's own for a workspace.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Options(_) => codes::INVALID_OPTIONS,
            Self::WorkingDirectory(_) => codes::WORKING_DIRECTORY,
            Self::Workspace(error) => error.code(),
        }
    }

    /// The refusal as the one protocol frame a host reads: empty identifier, the stable code and
    /// the application's details when it has any, in at most `limit` bytes. A refusal that would
    /// be longer, because a path or a message echoes a long input, keeps its code and loses text.
    #[must_use]
    pub fn refusal_line(&self, limit: usize) -> String {
        let details: Option<Value> = match self {
            Self::Workspace(error) => error.response().details,
            _ => None,
        };
        rejection("", self.code(), self, details, limit)
    }
}

/// Open the selected workspace. Opening reads the plan once, so a source that holds none is refused
/// here and never created, and a pinned clock applies to every query of the session.
pub fn start_blocking(options: &Options) -> Result<Application, StartError> {
    let here = std::env::current_dir().map_err(StartError::WorkingDirectory)?;
    open_workspace_blocking(
        &here,
        options.project.as_deref(),
        options.database.as_deref(),
    )
    .and_then(|app| app.plan_blocking().map(|_| app))
    .map(|app| app.with_query_clock(options.clock.map_or(QueryClock::System, QueryClock::Fixed)))
    .map_err(|error| StartError::Workspace(Box::new(error)))
}

#[cfg(test)]
mod tests;
