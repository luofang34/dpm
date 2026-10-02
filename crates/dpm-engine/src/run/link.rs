//! Deciding whether a committed project operation belongs to a run.

use super::{LinkRefusal, RunError};
use crate::{Command, Operation};
use chrono::{DateTime, Utc};
use dpm_model::{ActorId, LineageId, OperationId, RunRecord, WorkItemId, WorkspaceId};

/// What a committed operation records about itself, as the project store returned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationFacts {
    /// The operation's identity.
    pub id: OperationId,
    /// The principal that made it.
    pub actor: ActorId,
    /// When it committed.
    pub timestamp: DateTime<Utc>,
    /// The single task it acts on, if its command acts on one.
    pub work: Option<WorkItemId>,
    /// The workspace it changed.
    pub workspace: WorkspaceId,
    /// The lineage of the store that committed it.
    pub lineage: LineageId,
}

impl OperationFacts {
    /// Read the facts from a recorded operation.
    #[must_use]
    pub fn of(operation: &Operation, workspace: WorkspaceId, lineage: LineageId) -> Self {
        Self {
            id: operation.id,
            actor: operation.actor.clone(),
            timestamp: operation.timestamp,
            work: command_work(&operation.command),
            workspace,
            lineage,
        }
    }
}

/// The one task a command acts on; commands that act on none, or on many, name no task.
///
/// Every command is listed so that a new one must decide whether a run can perform it.
fn command_work(command: &Command) -> Option<WorkItemId> {
    match command {
        Command::RatifyContract { work }
        | Command::Reject { work, .. }
        | Command::Claim { work }
        | Command::Release { work, .. }
        | Command::Handoff { work, .. }
        | Command::Start { work, .. }
        | Command::Block { work, .. }
        | Command::Unblock { work }
        | Command::ReportProgress { work, .. }
        | Command::Submit { work, .. }
        | Command::Verify { work, .. }
        | Command::AttachArtifact { work, .. }
        | Command::UnlinkExternal { work, .. }
        | Command::RevalidateBasis { work, .. } => Some(*work),
        Command::LinkExternal(request) => Some(request.work),
        Command::ApplyChange { .. }
        | Command::WaiveDependency { .. }
        | Command::RestoreDependency { .. }
        | Command::Decide { .. } => None,
    }
}

/// Check that attributing `operation` to `record` cannot be wrong.
///
/// The operation must have been committed in the run's workspace and lineage, on the run's task, by
/// the run's executor, no earlier than the run started and, if the run ended at `ended_at`, no
/// later than that. A link is therefore evidence about what the run did, never a way to move
/// someone else's operation onto it after the fact.
pub fn check_link(
    record: &RunRecord,
    ended_at: Option<DateTime<Utc>>,
    operation: &OperationFacts,
) -> Result<(), RunError> {
    let refuse = |reason| {
        Err(RunError::LinkRefused {
            run: record.id,
            operation: operation.id,
            reason,
        })
    };
    if operation.workspace != record.contract.workspace_id {
        return refuse(LinkRefusal::OtherWorkspace);
    }
    if operation.lineage != record.contract.lineage_id {
        return refuse(LinkRefusal::OtherLineage);
    }
    if operation.work != Some(record.work) {
        return refuse(LinkRefusal::OtherWork);
    }
    if operation.actor != record.executor {
        return refuse(LinkRefusal::OtherActor);
    }
    if operation.timestamp < record.started_at {
        return refuse(LinkRefusal::BeforeStart);
    }
    if ended_at.is_some_and(|ended| operation.timestamp > ended) {
        return refuse(LinkRefusal::AfterEnd);
    }
    Ok(())
}
