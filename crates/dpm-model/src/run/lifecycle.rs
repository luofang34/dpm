//! Durable run lifecycle: the transitions that are facts rather than telemetry.

use crate::{ActorId, RunEventId, RunId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Where a run stands according to its last recorded transition.
///
/// This is lifecycle truth only. Whether anything has been heard from the run lately is a
/// separate, derived question (see `ObservedStatus`), so silence never edits this state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// The executor is working.
    Working,
    /// The executor waits for input, approval or another party.
    Waiting,
    /// The run ended without completing its work.
    Failed,
    /// The run was stopped before it finished.
    Interrupted,
    /// The executor finished. The work is unchanged: submission and verification are separate.
    Completed,
}

impl RunState {
    /// Whether no transition may follow.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Failed | Self::Interrupted | Self::Completed)
    }

    /// The lowercase word every adapter spells the state with.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Completed => "completed",
        }
    }
}

impl std::fmt::Display for RunState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.word())
    }
}

/// A state word no run has.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown run state {0:?}; expected working, waiting, failed, interrupted or completed")]
pub struct RunStateParseError(String);

impl FromStr for RunState {
    type Err = RunStateParseError;

    fn from_str(word: &str) -> Result<Self, Self::Err> {
        [
            Self::Working,
            Self::Waiting,
            Self::Failed,
            Self::Interrupted,
            Self::Completed,
        ]
        .into_iter()
        .find(|state| state.word() == word)
        .ok_or_else(|| RunStateParseError(word.into()))
    }
}

/// What a caller asks to record as a run's next lifecycle transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTransition {
    /// Transition identity and idempotency key, a version 7 UUID.
    pub id: RunEventId,
    /// The run that changes state.
    pub run: RunId,
    /// The state it moves to.
    pub to: RunState,
    /// Why, in the executor's words; a failure or interruption should say what happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// When the executor says it happened. It is a claim, never used to judge freshness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<DateTime<Utc>>,
}

/// One recorded lifecycle fact. A run's start is its first one, with the run's identity as its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleEvent {
    /// Transition identity.
    pub id: RunEventId,
    /// The run it belongs to.
    pub run: RunId,
    /// The state the run entered.
    pub state: RunState,
    /// Why, in the reporter's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// When the reporter says it happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<DateTime<Utc>>,
    /// The principal that reported it.
    pub recorded_by: ActorId,
    /// When DPM recorded it; the receipt time, from DPM's own clock.
    pub recorded_at: DateTime<Utc>,
}

/// A lifecycle fact addressed by its position in the lifecycle feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleEntry {
    /// Feed cursor value; lifecycle facts are never removed, so the feed has no gaps.
    pub sequence: u64,
    /// The fact.
    #[serde(flatten)]
    pub event: LifecycleEvent,
}

/// A bounded page of the lifecycle feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecyclePage {
    /// Facts in ascending feed order.
    pub entries: Vec<LifecycleEntry>,
    /// Cursor for the next request; unchanged for an empty page.
    pub next_after_sequence: u64,
    /// Newest sequence in the whole feed, so a client can tell it is caught up or was reset.
    pub head_sequence: u64,
}
