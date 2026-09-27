mod contract;
mod execution;
mod schedule;
pub use contract::WorkContract;
pub use execution::ExecutionRecord;
pub use schedule::ScheduleInputs;

use crate::{ActorId, EstimateError, Key, ProjectId, WorkItemId};
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Authoritative lifecycle for tasks; aggregate completion is a query projection.
pub enum WorkStatus {
    /// Work not yet approved for execution.
    #[default]
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
    /// Stable sibling position, independent of titles and keys.
    pub order: crate::SiblingOrder,
    /// Author-approved scope, requirements and applicability.
    pub contract: crate::WorkContract,
    /// Lifecycle and evidence recorded only by semantic commands.
    pub execution: crate::ExecutionRecord,
    /// Authoritative scheduling inputs; derived dates remain projections.
    pub schedule: crate::ScheduleInputs,
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
                .schedule
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

    /// When the task started: its recorded time, or an unknown time when it started before start
    /// times were recorded.
    ///
    /// Gate evaluation, resuming blocked work and progress reports all read this, so a legacy
    /// start is never forgotten by one of them while another still honours it.
    #[must_use]
    pub fn start_event(&self) -> Option<crate::EventTime> {
        let occurred = match self.execution.status {
            WorkStatus::InProgress
            | WorkStatus::Submitted
            | WorkStatus::Verified
            | WorkStatus::Done => true,
            WorkStatus::Blocked => self.execution.events.start_unrecorded,
            WorkStatus::Proposed | WorkStatus::Planned | WorkStatus::Claimed => false,
        };
        match self.execution.events.started_at {
            Some(at) => Some(crate::EventTime::Recorded(at)),
            None => occurred.then_some(crate::EventTime::Unrecorded),
        }
    }
}
