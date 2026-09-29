use super::{nonempty, task_mut};
use crate::EngineError;
use chrono::{DateTime, Utc};
use dpm_model::{
    ActorId, ActorKind, AttemptOutcome, Plan, ReviewRejection, WorkItemId, WorkStatus,
};

pub(super) fn ratify(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
) -> Result<(), EngineError> {
    if actor.kind == ActorKind::Agent {
        return Err(EngineError::ActorNotAllowed {
            actor: actor.clone(),
            action: "ratify a contract",
        });
    }
    let item = task_mut(plan, work)?;
    if item.execution.status != WorkStatus::Proposed {
        return Err(EngineError::InvalidTransition {
            work,
            status: item.execution.status,
        });
    }
    // The command's candidate validation enforces the complete executable contract atomically.
    item.execution.status = WorkStatus::Planned;
    Ok(())
}

pub(super) fn reject(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    nonempty(&work.to_string(), "rejection reason", reason)?;
    let item = task_mut(plan, work)?;
    if item.execution.status != WorkStatus::Submitted {
        return Err(EngineError::InvalidTransition {
            work,
            status: item.execution.status,
        });
    }
    // A former owner or releaser may have produced part of the result, so neither a handoff nor a
    // release makes it independent.
    if item.held_by(actor) {
        return Err(EngineError::ActorNotAllowed {
            actor: actor.clone(),
            action: "review work it has owned",
        });
    }
    super::refuse_evidence_author(plan, actor, work, "review work whose evidence it authored")?;
    let item = task_mut(plan, work)?;
    item.execution.status = WorkStatus::InProgress;
    // The rejected submission is no longer a finish claim; resubmission records a new time.
    item.execution.events.submitted_at = None;
    // The attempt stays in history so bases that relied on it remain visibly invalidated.
    if let Some(attempt) = item
        .execution
        .attempts
        .last_mut()
        .filter(|a| a.outcome == AttemptOutcome::Pending)
    {
        attempt.outcome = AttemptOutcome::Rejected {
            actor: actor.clone(),
            at,
            reason: reason.into(),
        };
    }
    item.execution.last_rejection = Some(ReviewRejection {
        actor: actor.clone(),
        at,
        reason: reason.into(),
    });
    Ok(())
}

#[cfg(test)]
mod tests;
