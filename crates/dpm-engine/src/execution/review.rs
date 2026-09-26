use super::{nonempty, task_mut};
use crate::EngineError;
use chrono::{DateTime, Utc};
use dpm_model::{ActorId, ActorKind, Plan, ReviewRejection, WorkItemId, WorkStatus};

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
    if item.status != WorkStatus::Proposed {
        return Err(EngineError::InvalidTransition {
            work,
            status: item.status,
        });
    }
    // The command's candidate validation enforces the complete executable contract atomically.
    item.status = WorkStatus::Planned;
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
    if item.status != WorkStatus::Submitted {
        return Err(EngineError::InvalidTransition {
            work,
            status: item.status,
        });
    }
    if item.owner.as_ref() == Some(actor) {
        return Err(EngineError::ActorNotAllowed {
            actor: actor.clone(),
            action: "review its own submission",
        });
    }
    item.status = WorkStatus::InProgress;
    item.last_rejection = Some(ReviewRejection {
        actor: actor.clone(),
        at,
        reason: reason.into(),
    });
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
