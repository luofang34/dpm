//! Recovery paths for a claim that should not stay with its owner. Neither command is a gated
//! transition: they change who executes the work, never how far it has progressed, so the shared
//! gate evaluator is not consulted and every recorded fact (events, attempts, basis, progress,
//! blocker, evidence) is carried over unchanged.

use super::{nonempty, owns, task_mut};
use crate::EngineError;
use chrono::{DateTime, Utc};
use dpm_model::{ActorId, ActorKind, ClaimRelease, Handoff, Plan, WorkItemId, WorkStatus};

/// Return an unstarted claim to Planned without an owner.
///
/// Only the owner may release, and only before the start: a claim reserves work without
/// executing it, so nothing downstream (SS/SF successors, provisional bases) has relied on it
/// and the reservation can be undone without losing a fact. Started work has recorded events a
/// release would orphan, so it moves only through an authorized handoff. The release is recorded
/// because a claimant may already have attached evidence, and so is never an independent reviewer.
pub(super) fn release(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    nonempty(&work.to_string(), "release reason", reason)?;
    let item = task_mut(plan, work)?;
    owns(item, actor)?;
    if item.start_event().is_some() {
        return Err(EngineError::AlreadyStarted(work));
    }
    if item.status != WorkStatus::Claimed || item.owner.as_ref() != Some(actor) {
        return Err(EngineError::InvalidTransition {
            work,
            status: item.status,
        });
    }
    item.owner = None;
    item.status = WorkStatus::Planned;
    item.releases.push(ClaimRelease {
        actor: actor.clone(),
        at,
        reason: reason.into(),
    });
    Ok(())
}

/// Transfer claimed, started or blocked work from its current owner to another actor.
///
/// Only a human or service may authorize a handoff: an agent that could reassign work could move
/// its own result to a collaborator and then review it, or take work from another executor. The
/// authorizer may be the current owner or the new one (a person taking over an interrupted
/// agent's work); independence is kept by the handoff record, because every review command refuses
/// any actor that ever held the work. Submitted work is refused: its pending attempt belongs to
/// the submitter, so a reviewer rejects it first and the rework can then be handed off.
pub(super) fn handoff(
    plan: &mut Plan,
    actor: &ActorId,
    request: HandoffRequest<'_>,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    let HandoffRequest {
        work,
        from,
        to,
        reason,
    } = request;
    nonempty(&work.to_string(), "handoff reason", reason)?;
    nonempty(&work.to_string(), "new owner name", &to.name)?;
    if actor.kind == ActorKind::Agent {
        return Err(EngineError::ActorNotAllowed {
            actor: actor.clone(),
            action: "authorize a handoff; release an unstarted claim or ask a human",
        });
    }
    let item = task_mut(plan, work)?;
    let held = matches!(
        item.status,
        WorkStatus::Claimed | WorkStatus::InProgress | WorkStatus::Blocked
    );
    let Some(owner) = item.owner.clone().filter(|_| held) else {
        return Err(EngineError::InvalidTransition {
            work,
            status: item.status,
        });
    };
    if owner != *from {
        return Err(EngineError::OwnerMismatch {
            work,
            expected: from.clone(),
            actual: owner,
        });
    }
    if owner == *to {
        return Err(EngineError::InvalidCommand {
            entity: work.to_string(),
            reason: format!("{to} already owns the work; hand it off to a different actor"),
        });
    }
    item.handoffs.push(Handoff {
        from: owner,
        to: to.clone(),
        actor: actor.clone(),
        at,
        reason: reason.into(),
    });
    item.owner = Some(to.clone());
    Ok(())
}

/// Borrowed fields of a handoff command.
pub(super) struct HandoffRequest<'a> {
    pub(super) work: WorkItemId,
    pub(super) from: &'a ActorId,
    pub(super) to: &'a ActorId,
    pub(super) reason: &'a str,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
