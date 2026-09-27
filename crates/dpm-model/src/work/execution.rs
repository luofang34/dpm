//! Recorded task lifecycle, ownership and evidence.

use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Execution records protected as one value by reviewed plan changes.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecord {
    /// Stored lifecycle; aggregate work has derived completion.
    pub status: WorkStatus,
    /// Owner-reported task execution progress; verification remains a separate condition.
    #[serde(default)]
    pub reported_progress_percent: u8,
    /// Attached evidence identifiers.
    pub artifact_ids: BTreeSet<ArtifactId>,
    /// Principal that claimed the task and owns submission.
    pub owner: Option<ActorId>,
    /// Authorized ownership transfers in order; append-only, so every earlier holder stays known.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handoffs: Vec<crate::Handoff>,
    /// Claims given back before a start, in order; append-only, so a releaser stays a holder.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub releases: Vec<crate::ClaimRelease>,
    /// Execution event times recorded by lifecycle commands.
    #[serde(default, skip_serializing_if = "crate::ExecutionEvents::is_empty")]
    pub events: crate::ExecutionEvents,
    /// Most recent independent rejection; retained across resubmission as review context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rejection: Option<ReviewRejection>,
    /// Every submission in order; reviews close attempts but never remove or renumber them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<crate::SubmissionAttempt>,
    /// Predecessor attempts this task's execution relies on; append-only, latest per edge applies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub basis: Vec<crate::DependencyBasis>,
    /// Non-empty reason while the task is blocked.
    pub block_reason: Option<String>,
}
