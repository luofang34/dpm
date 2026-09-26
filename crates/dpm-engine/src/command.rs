use chrono::{DateTime, Utc};
use dpm_model::{
    ActorId, Artifact, DecisionId, DependencyId, ExternalIdentity, ExternalLinkRole,
    ExternalReferenceId, ExternalState, Key, OperationId, ValidationError, WorkItemId, WorkKind,
    WorkStatus,
};
use dpm_schedule::ScheduleError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A semantic state change; adapters must use the engine to apply it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    /// Apply a reviewed plan proposal without bypassing execution history.
    ApplyChange {
        /// Full proposed graph at the observed revision, validated against protected state.
        plan: Box<dpm_model::Plan>,
        /// Human-readable purpose of the accepted scope change.
        reason: String,
    },
    /// Approve a proposed execution contract as a human or service.
    RatifyContract {
        /// Proposed task with a complete objective and acceptance criteria.
        work: WorkItemId,
    },
    /// Return submitted work to its owner with an independent review.
    Reject {
        /// Submitted task to return for rework.
        work: WorkItemId,
        /// Non-empty explanation of unmet acceptance.
        reason: String,
    },
    /// Reserve a ready task for the calling actor.
    Claim {
        /// Task to reserve.
        work: WorkItemId,
    },
    /// Suspend a task while preserving any existing owner.
    Block {
        /// Task to suspend.
        work: WorkItemId,
        /// Concrete blocker.
        reason: String,
    },
    /// Resume blocked work, preserving its owner.
    Unblock {
        /// Task to resume.
        work: WorkItemId,
    },
    /// Report task execution progress without accepting its result.
    ReportProgress {
        /// Owned task receiving the report.
        work: WorkItemId,
        /// Execution percentage from 0 through 100.
        percent: u8,
        /// Optional explanation or correction context.
        note: Option<String>,
    },
    /// Request independent verification of owned work.
    Submit {
        /// Task whose result is submitted.
        work: WorkItemId,
        /// Optional evidence summary.
        note: Option<String>,
    },
    /// Accept a submitted result as a different actor.
    Verify {
        /// Submitted task to verify.
        work: WorkItemId,
        /// Optional verification evidence.
        note: Option<String>,
    },
    /// Attach new evidence without replacing an existing artifact.
    AttachArtifact {
        /// Work receiving evidence.
        work: WorkItemId,
        /// New artifact attributed to the caller.
        artifact: Artifact,
    },
    /// Stop enforcing a soft constraint as a human or service, recording who, when and why.
    WaiveDependency {
        /// Soft constraint to set aside.
        dependency: DependencyId,
        /// Non-empty justification.
        reason: String,
    },
    /// Enforce a waived soft constraint again; the operation records who, when and why.
    RestoreDependency {
        /// Waived constraint to enforce again.
        dependency: DependencyId,
        /// Non-empty justification.
        reason: String,
    },
    /// Resolve an open gate.
    Decide {
        /// Gate to resolve.
        decision: DecisionId,
        /// Non-empty outcome.
        outcome: String,
    },
    /// Link work to an external tracker object without granting it local authority.
    LinkExternal(ExternalLinkRequest),
    /// Remove one work item's link to an external object; the last link removes the record.
    UnlinkExternal {
        /// Work whose link is removed.
        work: WorkItemId,
        /// Linked external reference.
        reference: ExternalReferenceId,
    },
}

/// Link request recording the resolved reference identifier so the operation log is explicit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalLinkRequest {
    /// Work receiving the link.
    pub work: WorkItemId,
    /// The recorded reference for this identity, or a new identifier when none exists.
    pub reference: ExternalReferenceId,
    /// Canonical provider identity.
    pub identity: ExternalIdentity,
    /// Display label; replaces the recorded label without changing identity.
    pub label: String,
    /// Optional display URL; replaces the recorded URL without changing identity.
    #[serde(default)]
    pub url: Option<String>,
    /// Relationship of the work to the external object.
    pub role: ExternalLinkRole,
    /// Optional external state reported by the caller; recorded as an observation only.
    #[serde(default)]
    pub observed: Option<ExternalState>,
}

/// Audit envelope for one successfully applied command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    /// Unique identity of this operation.
    pub id: OperationId,
    /// Revision the caller observed before applying the command.
    pub base_revision: u64,
    /// Next wrapping revision committed by the command.
    pub resulting_revision: u64,
    /// Principal making the change.
    pub actor: ActorId,
    /// Caller-supplied UTC operation time.
    pub timestamp: DateTime<Utc>,
    /// Semantic change whose result is stored.
    pub command: Command,
}

/// A rejected command or failed execution projection.
#[derive(Debug, Error)]
pub enum EngineError {
    /// A plan proposal no longer refers to the current revision.
    #[error("revision conflict: expected {expected}, current {actual}")]
    RevisionConflict {
        /// Revision the caller observed.
        expected: u64,
        /// Current revision.
        actual: u64,
    },
    /// A semantic difference could not be serialized.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Local principal policy refuses this action.
    #[error("actor {actor} is not permitted to {action}")]
    ActorNotAllowed {
        /// Principal requesting the operation.
        actor: ActorId,
        /// Refused action.
        action: &'static str,
    },
    /// Invalid authoritative input.
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// Invalid command field or target category.
    #[error("invalid command for {entity}: {reason}")]
    InvalidCommand {
        /// Identifier of the affected value.
        entity: String,
        /// Failed invariant.
        reason: String,
    },
    /// Referenced work does not exist.
    #[error("work item {0} does not exist")]
    MissingWorkItem(WorkItemId),
    /// Referenced temporal constraint does not exist.
    #[error("dependency {0} does not exist")]
    MissingDependency(DependencyId),
    /// Referenced gate does not exist.
    #[error("decision {0} does not exist")]
    MissingDecision(DecisionId),
    /// Execution commands target a grouping or condition rather than executable work.
    #[error("work item {work} is a {kind:?}, not a task; only tasks accept execution commands")]
    NotATask {
        /// Targeted work item.
        work: WorkItemId,
        /// Non-executable kind of the target.
        kind: WorkKind,
    },
    /// Execution prerequisites or lifecycle do not permit this command.
    #[error("work item {0} is not ready because execution prerequisites are incomplete")]
    NotReady(WorkItemId),
    /// A reported blocker prevents execution.
    #[error("work item {work} is blocked: {reason}")]
    Blocked {
        /// Affected work.
        work: WorkItemId,
        /// Recorded blocker.
        reason: String,
    },
    /// Another principal owns the task.
    #[error("work item {work} is owned by {owner}")]
    OwnedByAnother {
        /// Affected work.
        work: WorkItemId,
        /// Principal allowed to submit the result.
        owner: ActorId,
    },
    /// The lifecycle does not permit the requested transition.
    #[error("invalid state transition for {work} from {status:?}")]
    InvalidTransition {
        /// Affected work.
        work: WorkItemId,
        /// Current lifecycle.
        status: WorkStatus,
    },
    /// Verification requires a submitted result.
    #[error("cannot verify work {0} that has not been submitted")]
    NotSubmitted(WorkItemId),
    /// Verification must be independent of the owner.
    #[error("the submitting actor cannot verify its own work {0}")]
    SelfVerification(WorkItemId),
    /// A resolved gate cannot be resolved again.
    #[error("decision {0} is not open")]
    DecisionNotOpen(DecisionId),
    /// Referenced external object is not recorded in this workspace.
    #[error("external reference {0} does not exist")]
    MissingExternalReference(ExternalReferenceId),
    /// The work item has no link to the external reference.
    #[error("work item {work} has no link to external reference {reference}")]
    MissingExternalLink {
        /// Work named by the request.
        work: WorkItemId,
        /// External reference named by the request.
        reference: ExternalReferenceId,
    },
    /// The work item already links the external reference.
    #[error("work item {work} already links external reference {reference}; unlink it first")]
    DuplicateExternalLink {
        /// Work named by the request.
        work: WorkItemId,
        /// External reference already linked.
        reference: ExternalReferenceId,
    },
    /// Another work item already tracks the external object.
    #[error("{identity} is already tracked by {owner}; unlink it before transferring tracking")]
    TrackingOwned {
        /// External identity whose tracking is owned.
        identity: String,
        /// Key of the work that tracks it.
        owner: Key,
    },
    /// Schedule projection failed.
    #[error(transparent)]
    Schedule(#[from] ScheduleError),
}
