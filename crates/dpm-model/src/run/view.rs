//! What a run looks like when read: durable lifecycle beside derived observation.

use super::{ActivityTally, LifecycleEvent, RunLink, RunRecord, RunState};
use crate::{ActorId, WorkStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What an observer can honestly say about a run at one clock reading.
///
/// The first five follow the last recorded transition. `Stale` and `Unknown` are never stored:
/// they are derived from receipt times and attribution each time a view is computed, so a run
/// that goes silent is never labelled finished, and one that resumes needs no repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedStatus {
    /// Working, and heard from recently.
    Working,
    /// Waiting, and heard from recently.
    Waiting,
    /// Recorded as failed.
    Failed,
    /// Recorded as interrupted.
    Interrupted,
    /// Recorded as completed.
    Completed,
    /// Not finished according to its last transition, but nothing has been received for longer
    /// than the staleness bound. Its real state is unknown.
    Stale,
    /// Cannot be judged here: it was recorded under another history than this store continues,
    /// so whatever process it described is not known to be running in this one.
    Unknown,
}

/// Whether a run was recorded in the history the project store now continues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineageStatus {
    /// Recorded under the store's current lineage.
    Current,
    /// Recorded under another lineage, such as before a restore. Its attribution is unchanged
    /// and it is never reattached to this history.
    Foreign,
}

/// Why a run that is not finished no longer matches the task it executes.
///
/// Runs never move task ownership; a mismatch is reported so someone can decide what to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Orphan {
    /// The task is owned by someone other than the run's executor, or by no one.
    OwnerChanged {
        /// The task's current owner.
        owner: Option<ActorId>,
    },
    /// The task is no longer being executed.
    NotExecuting {
        /// The task's current lifecycle.
        status: WorkStatus,
    },
    /// The task is not in the plan this view was computed from.
    WorkMissing,
}

/// Everything a store holds about one run, before any clock or plan is applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSnapshot {
    /// The run as started.
    pub record: RunRecord,
    /// The latest lifecycle fact.
    pub last: LifecycleEvent,
    /// What the run's activity amounts to.
    pub activity: ActivityTally,
    /// Project operations explicitly linked to the run.
    pub operations: Vec<RunLink>,
}

/// A run with its derived observation, evaluated at one clock reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunView {
    /// The run as started.
    pub run: RunRecord,
    /// Lifecycle truth: the last recorded transition.
    pub state: RunState,
    /// When that transition was recorded.
    pub state_since: DateTime<Utc>,
    /// Detail the reporter gave with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_detail: Option<String>,
    /// What can be said about the run now; see [`ObservedStatus`].
    pub status: ObservedStatus,
    /// The latest time DPM received anything from the run, from DPM's own clock.
    pub last_receipt_at: DateTime<Utc>,
    /// When a working or waiting run turns stale if nothing more arrives; the instant a view
    /// should next be recomputed. Absent once the run is stale, unknown or finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_at: Option<DateTime<Utc>>,
    /// Activity received, retained and latest.
    pub activity: ActivityTally,
    /// Whether the run belongs to the store's current history.
    pub lineage: LineageStatus,
    /// Present when the run is unfinished but its task no longer matches it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orphan: Option<Orphan>,
    /// Project operations explicitly linked to the run.
    pub operations: Vec<RunLink>,
    /// The clock reading this view was computed at.
    pub evaluated_at: DateTime<Utc>,
}
