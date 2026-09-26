use crate::{ActorId, ArtifactId, EstimateError, Key, ProjectId, RequirementId, WorkItemId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Whether work is executable or a derived grouping/condition.
pub enum WorkKind {
    /// Non-executable container completed by all its children.
    WorkPackage,
    /// Executable work that can be claimed, submitted, and verified.
    Task,
    /// Zero-duration condition completed by verified prerequisites.
    Milestone,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
/// Human priority, separate from scheduling criticality.
pub enum Priority {
    /// Highest human priority.
    P0,
    /// High human priority.
    P1,
    #[default]
    /// Normal human priority and the default.
    P2,
    /// Low human priority.
    P3,
    /// Lowest human priority.
    P4,
}

impl Priority {
    #[must_use]
    /// Numeric ranking weight; larger values mean greater priority.
    pub fn rank(self) -> u8 {
        match self {
            Self::P0 => 4,
            Self::P1 => 3,
            Self::P2 => 2,
            Self::P3 => 1,
            Self::P4 => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Authoritative lifecycle for tasks; aggregate completion is a query projection.
pub enum WorkStatus {
    /// Work not yet approved for execution.
    Proposed,
    /// Approved work eligible for readiness evaluation.
    Planned,
    /// Work reserved by its owner.
    Claimed,
    /// Work being performed by its owner.
    InProgress,
    /// Work suspended with a reason.
    Blocked,
    /// Owner reports completion and requests independent verification.
    Submitted,
    /// Independent verification succeeded.
    Verified,
    /// Finalized completed work.
    Done,
}

impl WorkStatus {
    #[must_use]
    /// Whether a task has passed verification.
    pub fn satisfies_dependency(self) -> bool {
        matches!(self, Self::Verified | Self::Done)
    }

    #[must_use]
    /// Whether a task has completed execution.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Verified | Self::Done)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Observable evidence required for submitted work.
#[serde(deny_unknown_fields)]
pub struct AcceptanceCriterion {
    /// Concrete acceptance condition.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
/// Ordered non-negative duration estimates in elapsed hours.
#[serde(deny_unknown_fields)]
pub struct ThreePointEstimate {
    /// Optimistic duration in elapsed hours.
    pub optimistic_hours: f64,
    /// Most-likely duration in elapsed hours.
    pub likely_hours: f64,
    /// Pessimistic duration in elapsed hours.
    pub pessimistic_hours: f64,
}

impl ThreePointEstimate {
    /// Reject non-finite, negative, or unordered duration bounds.
    pub fn validate(self) -> Result<(), EstimateError> {
        if !self.optimistic_hours.is_finite()
            || !self.likely_hours.is_finite()
            || !self.pessimistic_hours.is_finite()
        {
            return Err(EstimateError::NonFinite);
        }
        if self.optimistic_hours < 0.0 {
            return Err(EstimateError::Negative);
        }
        if self.optimistic_hours > self.likely_hours || self.likely_hours > self.pessimistic_hours {
            return Err(EstimateError::Unordered);
        }
        Ok(())
    }

    #[must_use]
    /// Weighted PERT estimate in elapsed hours.
    pub fn pert_expected_hours(self) -> f64 {
        self.likely_hours
            + (self.optimistic_hours - self.likely_hours) / 6.0
            + (self.pessimistic_hours - self.likely_hours) / 6.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Execution contract and authoritative lifecycle for a unit of work.
#[serde(deny_unknown_fields)]
pub struct WorkItem {
    /// Stable entity identity; it must match its containing map key.
    pub id: WorkItemId,
    /// Human-readable key, unique in this entity category.
    pub key: Key,
    /// Project that owns this entity.
    pub project: ProjectId,
    /// Optional parent; containment must be acyclic.
    pub parent: Option<WorkItemId>,
    /// Domain category of this value.
    pub kind: WorkKind,
    /// Non-empty human-readable title.
    pub title: String,
    /// Observable purpose of this work.
    pub objective: String,
    /// Evidence requirements for executable work.
    pub acceptance: Vec<AcceptanceCriterion>,
    /// Optional ordered procedure and scope; absent in plans that do not supply instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<crate::WorkInstructions>,
    /// Stored lifecycle; aggregate work has derived completion.
    pub status: WorkStatus,
    /// Owner-reported task execution progress; verification remains a separate condition.
    #[serde(default)]
    pub reported_progress_percent: u8,
    /// Human-assigned importance.
    pub priority: Priority,
    /// Optional duration uncertainty; absent estimates contribute zero hours.
    pub estimate: Option<ThreePointEstimate>,
    /// Capabilities required of a recommended worker.
    pub capabilities: BTreeSet<String>,
    /// Requirements implemented by this work.
    pub requirement_ids: BTreeSet<RequirementId>,
    /// Attached evidence identifiers.
    pub artifact_ids: BTreeSet<ArtifactId>,
    /// Principal that claimed the task and owns submission.
    pub owner: Option<ActorId>,
    /// Explicit read/write needs; an empty set permits work without repository resources.
    pub resources: Vec<crate::ResourceRequirement>,
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
    /// Decision option this work and its descendants apply to; absent means unconditional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<crate::WorkCondition>,
    /// How a task or milestone treats incoming constraints from work a choice excluded.
    #[serde(default, skip_serializing_if = "crate::JoinPolicy::is_default")]
    pub join: crate::JoinPolicy,
}

/// Independent review explaining why submitted work needs another attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRejection {
    /// Principal who rejected the result.
    pub actor: ActorId,
    /// Caller-supplied UTC review time.
    pub at: chrono::DateTime<chrono::Utc>,
    /// Concrete unmet acceptance or evidence requirement.
    pub reason: String,
}

impl WorkItem {
    #[must_use]
    /// Expected task duration; containers and milestones contribute zero.
    pub fn expected_duration_hours(&self) -> f64 {
        match self.kind {
            WorkKind::Task => self
                .estimate
                .map_or(0.0, ThreePointEstimate::pert_expected_hours),
            WorkKind::WorkPackage | WorkKind::Milestone => 0.0,
        }
    }

    #[must_use]
    /// Whether this work kind supports task lifecycle commands.
    pub fn is_executable(&self) -> bool {
        self.kind == WorkKind::Task
    }
}
