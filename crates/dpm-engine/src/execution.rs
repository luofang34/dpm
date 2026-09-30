use crate::{Command, EngineError, EntityChange, Operation};
use chrono::{DateTime, Utc};
use dpm_model::{ActorId, Artifact, DecisionStatus, OperationId, Plan, WorkItem, WorkItemId};
use lifecycle::{block, claim, report_progress, start, submit, unblock, verify};

/// Apply one validated semantic command atomically and return its audit operation.
///
/// Errors leave every field, including the revision, unchanged. Revisions wrap at `u64::MAX`.
/// The caller supplies the operation identity so replaying a recorded operation reproduces it
/// exactly; the engine itself holds no source of randomness.
pub fn apply_command(
    plan: &mut Plan,
    actor: ActorId,
    command: Command,
    timestamp: DateTime<Utc>,
    id: OperationId,
) -> Result<Operation, EngineError> {
    plan.validate()?;
    nonempty(&actor.to_string(), "actor name", &actor.name)?;
    let mut candidate = plan.clone();
    execute(&mut candidate, &actor, &command, timestamp)?;
    candidate.revision = plan.revision.wrapping_add(1);
    candidate.validate()?;
    let operation = Operation {
        id,
        base_revision: plan.revision,
        resulting_revision: candidate.revision,
        actor,
        timestamp,
        command,
    };
    *plan = candidate;
    Ok(operation)
}

fn execute(
    plan: &mut Plan,
    actor: &ActorId,
    command: &Command,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    match command {
        Command::ApplyChange { changes, reason } => apply_change(plan, actor, changes, reason, at),
        Command::RatifyContract { work } => review::ratify(plan, actor, *work),
        Command::Reject { work, reason } => review::reject(plan, actor, *work, reason, at),
        Command::Claim { work } => claim(plan, actor, *work, at),
        Command::Release { work, reason } => ownership::release(plan, actor, *work, reason, at),
        Command::Handoff {
            work,
            from,
            to,
            reason,
        } => ownership::handoff(
            plan,
            actor,
            ownership::HandoffRequest {
                work: *work,
                from,
                to,
                reason,
            },
            at,
        ),
        Command::Start { work, occurred_at } => start(plan, actor, *work, *occurred_at, at),
        Command::Block { work, reason } => block(plan, actor, *work, reason),
        Command::Unblock { work } => unblock(plan, actor, *work),
        Command::ReportProgress { work, percent, .. } => {
            report_progress(plan, actor, *work, *percent)
        }
        Command::Submit {
            work, occurred_at, ..
        } => submit(plan, actor, *work, *occurred_at, at),
        Command::Verify {
            work, occurred_at, ..
        } => verify(plan, actor, *work, *occurred_at, at),
        Command::AttachArtifact { work, artifact } => attach(plan, actor, *work, artifact),
        Command::LinkExternal(request) => tracking::link(plan, actor, request, at),
        Command::UnlinkExternal { work, reference } => tracking::unlink(plan, *work, *reference),
        Command::WaiveDependency { dependency, reason } => {
            waiver::waive(plan, actor, *dependency, reason, at)
        }
        Command::RestoreDependency { dependency, reason } => {
            waiver::restore(plan, actor, *dependency, reason)
        }
        Command::RevalidateBasis {
            work,
            dependency,
            attempt,
            reason,
        } => revalidation::revalidate(plan, actor, *work, *dependency, *attempt, reason, at),
        Command::Decide { decision, outcome } => {
            choice::decide(plan, actor, *decision, outcome, at)
        }
    }
}

fn apply_change(
    plan: &mut Plan,
    actor: &ActorId,
    changes: &[EntityChange],
    reason: &str,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    crate::change::authorize(actor, reason)?;
    let mut candidate = crate::patch(plan, changes)?;
    let preview = crate::propose_change(plan, &candidate)?;
    if preview.changes.is_empty() {
        return Err(EngineError::InvalidCommand {
            entity: "plan".into(),
            reason: "proposal has no semantic changes".into(),
        });
    }
    // Only the canonical difference is recorded, so the log reads the same as `plan diff` and a
    // reordered or padded change list cannot replay to the same state under another description.
    if preview.changes != changes {
        return Err(EngineError::InvalidCommand {
            entity: "plan".into(),
            reason: "changes must be the canonical difference plan_change computes".into(),
        });
    }
    // Applying the review is what makes a replacement's choice, so, as with decide, the command
    // records its own time; the proposal cannot carry one (see propose_change).
    for decision in candidate
        .decisions
        .values_mut()
        .filter(|d| !plan.decisions.contains_key(&d.id) && d.status != DecisionStatus::Open)
    {
        decision.resolved_at = Some(at);
    }
    crate::change::refuse_own_relaxation(plan, &candidate, actor, at)?;
    *plan = candidate;
    Ok(())
}

fn task_mut(plan: &mut Plan, work: WorkItemId) -> Result<&mut WorkItem, EngineError> {
    let item = plan
        .work_items
        .get_mut(&work)
        .ok_or(EngineError::MissingWorkItem(work))?;
    if !item.is_executable() {
        return Err(EngineError::NotATask {
            work,
            kind: item.kind,
        });
    }
    Ok(item)
}

fn owns(item: &WorkItem, actor: &ActorId) -> Result<(), EngineError> {
    if let Some(owner) = &item.execution.owner
        && owner != actor
    {
        return Err(EngineError::OwnedByAnother {
            work: item.id,
            owner: owner.clone(),
        });
    }
    Ok(())
}

/// Refuse a reviewer that wrote evidence attached to the task: its review would judge its own
/// contribution. Only the task's own artifacts count; evidence on a containing package is context.
fn refuse_evidence_author(
    plan: &Plan,
    actor: &ActorId,
    work: WorkItemId,
    action: &'static str,
) -> Result<(), EngineError> {
    let item = plan
        .work_items
        .get(&work)
        .ok_or(EngineError::MissingWorkItem(work))?;
    let authored = item
        .execution
        .artifact_ids
        .iter()
        .filter_map(|id| plan.artifacts.get(id))
        .filter(|artifact| !is_planning_source(artifact))
        .any(|artifact| artifact.created_by == *actor);
    if authored {
        return Err(EngineError::ActorNotAllowed {
            actor: actor.clone(),
            action,
        });
    }
    Ok(())
}

fn attach(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
    artifact: &Artifact,
) -> Result<(), EngineError> {
    let item = plan
        .work_items
        .get(&work)
        .ok_or(EngineError::MissingWorkItem(work))?;
    owns(item, actor)?;
    // Evidence authors cannot review the task, so only the executor attaches evidence to it.
    if item.is_executable() && item.execution.owner.is_none() {
        return Err(EngineError::ActorNotAllowed {
            actor: actor.clone(),
            action: "attach evidence to an unclaimed task; claim it first",
        });
    }
    // Planning sources come from reviewed plans; labelling evidence as one would hide its author.
    if is_planning_source(artifact) {
        return Err(EngineError::InvalidCommand {
            entity: artifact.id.to_string(),
            reason: "planning sources arrive through reviewed plan changes, not as evidence".into(),
        });
    }
    if artifact.created_by != *actor || plan.artifacts.contains_key(&artifact.id) {
        return Err(EngineError::InvalidCommand {
            entity: artifact.id.to_string(),
            reason: "artifact creator must match actor and id must be new".into(),
        });
    }
    let item = plan
        .work_items
        .get_mut(&work)
        .ok_or(EngineError::MissingWorkItem(work))?;
    item.execution.artifact_ids.insert(artifact.id);
    plan.artifacts.insert(artifact.id, artifact.clone());
    Ok(())
}

/// Plan-authored context (design documents, requirements sources) rather than execution evidence.
fn is_planning_source(artifact: &Artifact) -> bool {
    artifact
        .metadata
        .get("role")
        .is_some_and(|role| role == PLANNING_SOURCE_ROLE)
}

const PLANNING_SOURCE_ROLE: &str = "planning_source";

fn nonempty(entity: &str, field: &str, value: &str) -> Result<(), EngineError> {
    if value.trim().is_empty() {
        return Err(EngineError::InvalidCommand {
            entity: entity.into(),
            reason: format!("{field} must not be empty"),
        });
    }
    Ok(())
}

mod choice;
mod lifecycle;
mod occurrence;
mod ownership;
mod revalidation;
mod review;
mod tracking;
mod waiver;

#[cfg(test)]
mod tests;
