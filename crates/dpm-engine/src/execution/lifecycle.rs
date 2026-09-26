//! Task lifecycle commands. Each relation-gated transition asks the shared gate evaluator at the
//! operation's own timestamp and records that timestamp as the transition's event.

use super::{nonempty, owns, task_mut};
use crate::{EngineError, Transition, gates};
use chrono::{DateTime, Utc};
use dpm_model::{ActorId, Plan, Timeline, WorkItemId, WorkStatus};

/// Reject the transition unless every gate the query views report for it is satisfied at `at`.
fn permit(
    plan: &Plan,
    work: WorkItemId,
    transition: Transition,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    let item = plan
        .work_items
        .get(&work)
        .ok_or(EngineError::MissingWorkItem(work))?;
    let report = gates::evaluate(plan, item, transition, &Timeline::at(plan, at));
    if report.ready {
        return Ok(());
    }
    if let Some(reason) = &item.block_reason {
        return Err(EngineError::Blocked {
            work,
            reason: reason.clone(),
        });
    }
    Err(EngineError::NotReady {
        work,
        transition,
        unmet: report.unmet,
    })
}

fn require_status(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
    expected: WorkStatus,
) -> Result<(), EngineError> {
    let item = task_mut(plan, work)?;
    owns(item, actor)?;
    if item.status == WorkStatus::Claimed && expected == WorkStatus::InProgress {
        return Err(EngineError::NotStarted(work));
    }
    if item.status != expected || item.owner.as_ref() != Some(actor) {
        return Err(EngineError::InvalidTransition {
            work,
            status: item.status,
        });
    }
    Ok(())
}

pub(super) fn claim(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    // The kind check precedes readiness so a non-task never reports a misleading "not ready".
    task_mut(plan, work)?;
    permit(plan, work, Transition::Claim, at)?;
    let item = task_mut(plan, work)?;
    item.owner = Some(actor.clone());
    item.status = WorkStatus::Claimed;
    Ok(())
}

/// Begin execution; only this event releases SS and SF successors.
pub(super) fn start(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    require_status(plan, actor, work, WorkStatus::Claimed)?;
    permit(plan, work, Transition::Start, at)?;
    let item = task_mut(plan, work)?;
    item.status = WorkStatus::InProgress;
    item.events.started_at = Some(at);
    Ok(())
}

pub(super) fn block(
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

/// Resume blocked work where it stopped: started work stays started, reservations stay reserved.
pub(super) fn unblock(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
) -> Result<(), EngineError> {
    let item = task_mut(plan, work)?;
    owns(item, actor)?;
    if item.status != WorkStatus::Blocked {
        return Err(EngineError::InvalidTransition {
            work: item.id,
            status: item.status,
        });
    }
    item.block_reason = None;
    item.status = if item.events.started_at.is_some() {
        WorkStatus::InProgress
    } else if item.owner.is_some() {
        WorkStatus::Claimed
    } else {
        WorkStatus::Planned
    };
    Ok(())
}

pub(super) fn report_progress(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
    percent: u8,
) -> Result<(), EngineError> {
    let item = task_mut(plan, work)?;
    owns(item, actor)?;
    let started_blocked = item.status == WorkStatus::Blocked && item.events.started_at.is_some();
    if !started_blocked {
        require_status(plan, actor, work, WorkStatus::InProgress)?;
    }
    let item = task_mut(plan, work)?;
    if item.owner.as_ref() != Some(actor) {
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
    Ok(())
}

pub(super) fn submit(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    require_status(plan, actor, work, WorkStatus::InProgress)?;
    permit(plan, work, Transition::Submit, at)?;
    let item = task_mut(plan, work)?;
    item.status = WorkStatus::Submitted;
    item.events.submitted_at = Some(at);
    Ok(())
}

pub(super) fn verify(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    let item = task_mut(plan, work)?;
    if item.status != WorkStatus::Submitted {
        return Err(EngineError::NotSubmitted(work));
    }
    if item.owner.as_ref() == Some(actor) {
        return Err(EngineError::SelfVerification(work));
    }
    permit(plan, work, Transition::Verify, at)?;
    let item = task_mut(plan, work)?;
    item.status = WorkStatus::Verified;
    item.events.verified_at = Some(at);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
