//! The values a session takes and returns: its bounds, how it begins, and what it reports.

use crate::{
    intake::{Tally, Terminal},
    process::Stopped,
    sink::SinkError,
};
use dpm_model::RunId;
use std::time::Duration;
use thiserror::Error;

/// How a batch write is retried when the store is busy.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    /// Attempts per write, at least one.
    pub attempts: u32,
    /// Pause before the second attempt; each later one waits proportionally longer.
    pub backoff: Duration,
    /// The longest all attempts of one write may take together, pauses included.
    pub budget: Duration,
}

/// What bounds and shapes one session.
#[derive(Debug, Clone)]
pub struct Turn {
    /// The provider session this turn belongs to, which its announcement must match.
    pub session: String,
    /// The prompt sent as the one user message. It is never recorded.
    pub prompt: String,
    /// The longest the whole session may take.
    pub deadline: Duration,
    /// How long, after the provider acknowledges `initialize`, to wait for it to announce its
    /// session before recording the run without that announcement.
    pub announce_wait: Duration,
    /// Most records per write, at most the application's batch bound.
    pub batch: usize,
    /// How long each step of stopping the provider may take.
    pub grace: Duration,
    /// How busy-store writes are retried.
    pub retry: Retry,
}

/// What recording a run needs, once the run exists.
#[derive(Debug)]
pub struct Begun<S> {
    /// Where the run's records go.
    pub sink: S,
    /// The run.
    pub run: RunId,
}

/// Why a run could not be recorded, with the failure underneath kept as its source.
#[derive(Debug, Error)]
#[error("{context}")]
pub struct BeginError {
    /// What was being done.
    pub context: &'static str,
    /// The underlying failure.
    #[source]
    pub source: Box<dyn std::error::Error + Send + Sync>,
}

impl BeginError {
    /// A failure of `context` caused by `source`.
    pub fn new(
        context: &'static str,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self {
            context,
            source: source.into(),
        }
    }
}

/// Why the provider never acknowledged the handshake, so the run was not recorded and the prompt
/// was not sent.
#[derive(Debug, Error)]
pub enum HandshakeError {
    /// Its output ended, or its process exited, without a success response to `initialize`.
    #[error("the provider's output ended before it acknowledged initialize")]
    Ended,
    /// It sent this many frames and none acknowledged `initialize`.
    #[error("the provider sent {frames} frames and none acknowledged initialize")]
    Silent {
        /// How many frames were read.
        frames: usize,
    },
    /// The time bound was reached first.
    #[error("the time bound was reached before the provider acknowledged initialize")]
    Deadline,
    /// The operator asked to stop first.
    #[error("the run was cancelled before the provider acknowledged initialize")]
    Cancelled,
    /// The provider refused, or broke the protocol before acknowledging.
    #[error("{0}")]
    Protocol(String),
}

/// A session that could not begin: why, and how stopping its provider went, so that cleanup
/// that did not succeed is never mistaken for cleanup that did.
#[derive(Debug)]
pub struct DriveFailure {
    /// Why the run could not be recorded.
    pub error: BeginError,
    /// How stopping the provider went.
    pub stopped: Stopped,
}

/// What a session did.
#[derive(Debug)]
pub struct Report {
    /// The run.
    pub run: RunId,
    /// The terminal state the evidence supports.
    pub terminal: Terminal,
    /// Whether the terminal transition was durably recorded.
    pub terminal_recorded: bool,
    /// Why recording failed, when it did.
    pub sink_failure: Option<SinkError>,
    /// What the intake counted.
    pub tally: Tally,
    /// How stopping the provider went.
    pub stopped: Stopped,
    /// Bytes the provider wrote to standard error, none of which were kept.
    pub stderr_bytes: u64,
    /// The most output messages ever waiting for the intake.
    pub pending_high_water: usize,
    /// Whether the replay window covered every record of the run.
    pub window_complete: bool,
}
