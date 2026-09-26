use crate::{
    Command, EngineError, Operation, decisions_resolved, dependencies_satisfied, is_ready,
};
use chrono::{DateTime, Utc};
use dpm_model::{
    ActorId, Artifact, DecisionStatus, OperationId, Plan, WorkItem, WorkItemId, WorkStatus,
};

/// Apply one validated semantic command atomically and return its audit operation.
///
/// Errors leave every field, including the revision, unchanged. Revisions wrap at `u64::MAX`.
pub fn apply_command(
    plan: &mut Plan,
    actor: ActorId,
    command: Command,
    timestamp: DateTime<Utc>,
) -> Result<Operation, EngineError> {
    plan.validate()?;
    nonempty(&actor.to_string(), "actor name", &actor.name)?;
    let mut candidate = plan.clone();
    execute(&mut candidate, &actor, &command, timestamp)?;
    candidate.revision = plan.revision.wrapping_add(1);
    candidate.validate()?;
    let operation = Operation {
        id: OperationId::new(),
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
        Command::RatifyContract { work } => review::ratify(plan, actor, *work),
        Command::Reject { work, reason } => review::reject(plan, actor, *work, reason, at),
        Command::Claim { work } => claim(plan, actor, *work),
        Command::Block { work, reason } => block(plan, actor, *work, reason),
        Command::Unblock { work } => {
            let item = task_mut(plan, *work)?;
            owns(item, actor)?;
            if item.status != WorkStatus::Blocked {
                return Err(EngineError::InvalidTransition {
                    work: item.id,
                    status: item.status,
                });
            }
            item.block_reason = None;
            item.status = if item.owner.is_some() {
                WorkStatus::Claimed
            } else {
                WorkStatus::Planned
            };
            Ok(())
        }
        Command::ReportProgress { work, percent, .. } => {
            report_progress(plan, actor, *work, *percent)
        }
        Command::Submit { work, .. } => submit(plan, actor, *work),
        Command::Verify { work, .. } => verify(plan, actor, *work),
        Command::AttachArtifact { work, artifact } => attach(plan, actor, *work, artifact),
        Command::Decide { decision, outcome } => {
            nonempty(&decision.to_string(), "outcome", outcome)?;
            let gate = plan
                .decisions
                .get_mut(decision)
                .ok_or(EngineError::MissingDecision(*decision))?;
            if gate.status != DecisionStatus::Open {
                return Err(EngineError::DecisionNotOpen(*decision));
            }
            gate.status = DecisionStatus::Decided;
            gate.outcome = Some(outcome.clone());
            Ok(())
        }
    }
}

fn task_mut(plan: &mut Plan, work: WorkItemId) -> Result<&mut WorkItem, EngineError> {
    let item = plan
        .work_items
        .get_mut(&work)
        .ok_or(EngineError::MissingWorkItem(work))?;
    if !item.is_executable() {
        return Err(EngineError::InvalidCommand {
            entity: work.to_string(),
            reason: "only tasks support execution commands".into(),
        });
    }
    Ok(item)
}

fn owns(item: &WorkItem, actor: &ActorId) -> Result<(), EngineError> {
    if let Some(owner) = &item.owner
        && owner != actor
    {
        return Err(EngineError::OwnedByAnother {
            work: item.id,
            owner: owner.clone(),
        });
    }
    Ok(())
}

fn claim(plan: &mut Plan, actor: &ActorId, work: WorkItemId) -> Result<(), EngineError> {
    let item = plan
        .work_items
        .get(&work)
        .ok_or(EngineError::MissingWorkItem(work))?;
    if !is_ready(plan, item) {
        if let Some(reason) = &item.block_reason {
            return Err(EngineError::Blocked {
                work,
                reason: reason.clone(),
            });
        }
        return Err(EngineError::NotReady(work));
    }
    let item = task_mut(plan, work)?;
    item.owner = Some(actor.clone());
    item.status = WorkStatus::Claimed;
    Ok(())
}

fn block(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
    reason: &str,
) -> Result<(), EngineError> {
    nonempty(&work.to_string(), "block reason", reason)?;
    let item = task_mut(plan, work)?;
    owns(item, actor)?;
    if !matches!(
        item.status,
        WorkStatus::Planned | WorkStatus::Claimed | WorkStatus::InProgress
    ) {
        return Err(EngineError::InvalidTransition {
            work: item.id,
            status: item.status,
        });
    }
    item.block_reason = Some(reason.into());
    item.status = WorkStatus::Blocked;
    Ok(())
}

fn report_progress(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
    percent: u8,
) -> Result<(), EngineError> {
    let item = task_mut(plan, work)?;
    owns(item, actor)?;
    if item.owner.as_ref() != Some(actor)
        || !matches!(
            item.status,
            WorkStatus::Claimed | WorkStatus::InProgress | WorkStatus::Blocked
        )
    {
        return Err(EngineError::InvalidTransition {
            work,
            status: item.status,
        });
    }
    if percent > 100 {
        return Err(EngineError::InvalidCommand {
            entity: work.to_string(),
            reason: "progress must be between 0 and 100".into(),
        });
    }
    item.reported_progress_percent = percent;
    if item.status != WorkStatus::Blocked {
        item.status = WorkStatus::InProgress;
    }
    Ok(())
}

fn submit(plan: &mut Plan, actor: &ActorId, work: WorkItemId) -> Result<(), EngineError> {
    let item = task_mut(plan, work)?;
    owns(item, actor)?;
    if !matches!(item.status, WorkStatus::Claimed | WorkStatus::InProgress) {
        return Err(EngineError::InvalidTransition {
            work: item.id,
            status: item.status,
        });
    }
    item.status = WorkStatus::Submitted;
    Ok(())
}

fn verify(plan: &mut Plan, actor: &ActorId, work: WorkItemId) -> Result<(), EngineError> {
    if !dependencies_satisfied(plan, work) || !decisions_resolved(plan, work) {
        return Err(EngineError::NotReady(work));
    }
    let item = task_mut(plan, work)?;
    if item.status != WorkStatus::Submitted {
        return Err(EngineError::NotSubmitted(work));
    }
    if item.owner.as_ref() == Some(actor) {
        return Err(EngineError::SelfVerification(work));
    }
    item.status = WorkStatus::Verified;
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
    item.artifact_ids.insert(artifact.id);
    plan.artifacts.insert(artifact.id, artifact.clone());
    Ok(())
}

fn nonempty(entity: &str, field: &str, value: &str) -> Result<(), EngineError> {
    if value.trim().is_empty() {
        return Err(EngineError::InvalidCommand {
            entity: entity.into(),
            reason: format!("{field} must not be empty"),
        });
    }
    Ok(())
}

mod review;

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
