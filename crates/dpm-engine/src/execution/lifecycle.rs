//! Task lifecycle commands. Each relation-gated transition asks the shared gate evaluator at the
//! operation's own timestamp and records that timestamp as the transition's event.

use super::{nonempty, owns, task_mut};
use crate::{EngineError, GateReport, Transition, gates};
use chrono::{DateTime, Utc};
use dpm_model::{
    ActorId, AttemptOutcome, BasisSource, DependencyBasis, Plan, SubmissionAttempt, Timeline,
    WorkItemId, WorkStatus,
};

/// Reject the transition unless every gate the query views report for it is satisfied at `at`.
fn permit(
    plan: &Plan,
    work: WorkItemId,
    transition: Transition,
    at: DateTime<Utc>,
) -> Result<GateReport, EngineError> {
    let item = plan
        .work_items
        .get(&work)
        .ok_or(EngineError::MissingWorkItem(work))?;
    let report = gates::evaluate(plan, item, transition, &Timeline::at(plan, at));
    if report.ready {
        return Ok(report);
    }
    if let Some(reason) = &item.execution.block_reason {
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
    if item.execution.status == WorkStatus::Claimed && expected == WorkStatus::InProgress {
        return Err(EngineError::NotStarted(work));
    }
    if item.execution.status != expected || item.execution.owner.as_ref() != Some(actor) {
        return Err(EngineError::InvalidTransition {
            work,
            status: item.execution.status,
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
    item.execution.owner = Some(actor.clone());
    item.execution.status = WorkStatus::Claimed;
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
    let report = permit(plan, work, Transition::Start, at)?;
    let item = task_mut(plan, work)?;
    item.execution.status = WorkStatus::InProgress;
    item.execution.events.started_at = Some(at);
    // The start relies on exactly the attempts the shared evaluator released it on.
    item.execution.basis.extend(
        report
            .provisional
            .into_iter()
            .map(|release| DependencyBasis {
                dependency: release.dependency,
                predecessor: release.predecessor,
                attempt: release.attempt,
                recorded_at: at,
                source: BasisSource::Start,
            }),
    );
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
        item.execution.status,
        WorkStatus::Planned | WorkStatus::Claimed | WorkStatus::InProgress
    ) {
        return Err(EngineError::InvalidTransition {
            work: item.id,
            status: item.execution.status,
        });
    }
    // The start of work begun before start times were recorded survives only in its lifecycle.
    if item.execution.status == WorkStatus::InProgress && item.execution.events.started_at.is_none()
    {
        item.execution.events.start_unrecorded = true;
    }
    item.execution.block_reason = Some(reason.into());
    item.execution.status = WorkStatus::Blocked;
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
    if item.execution.status != WorkStatus::Blocked {
        return Err(EngineError::InvalidTransition {
            work: item.id,
            status: item.execution.status,
        });
    }
    item.execution.block_reason = None;
    item.execution.status = if item.start_event().is_some() {
        WorkStatus::InProgress
    } else if item.execution.owner.is_some() {
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
    let started_blocked =
        item.execution.status == WorkStatus::Blocked && item.start_event().is_some();
    if !started_blocked {
        require_status(plan, actor, work, WorkStatus::InProgress)?;
    }
    let item = task_mut(plan, work)?;
    if item.execution.owner.as_ref() != Some(actor) {
        return Err(EngineError::InvalidTransition {
            work,
            status: item.execution.status,
        });
    }
    if percent > 100 {
        return Err(EngineError::InvalidCommand {
            entity: work.to_string(),
            reason: "progress must be between 0 and 100".into(),
        });
    }
    item.execution.reported_progress_percent = percent;
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
    item.execution.status = WorkStatus::Submitted;
    item.execution.events.submitted_at = Some(at);
    let number = item
        .execution
        .attempts
        .last()
        .map_or(1, |a| a.number.wrapping_add(1));
    item.execution.attempts.push(SubmissionAttempt {
        number,
        submitted_at: at,
        outcome: AttemptOutcome::Pending,
    });
    Ok(())
}

pub(super) fn verify(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    let item = task_mut(plan, work)?;
    if item.execution.status != WorkStatus::Submitted {
        return Err(EngineError::NotSubmitted(work));
    }
    if item.execution.owner.as_ref() == Some(actor) {
        return Err(EngineError::SelfVerification(work));
    }
    if item.held_by(actor) {
        return Err(EngineError::ActorNotAllowed {
            actor: actor.clone(),
            action: "verify work it held before a handoff or release",
        });
    }
    super::refuse_evidence_author(plan, actor, work, "verify work whose evidence it authored")?;
    permit(plan, work, Transition::Verify, at)?;
    let item = task_mut(plan, work)?;
    item.execution.status = WorkStatus::Verified;
    item.execution.events.verified_at = Some(at);
    // A legacy submission has no attempt record to close.
    if let Some(attempt) = item
        .execution
        .attempts
        .last_mut()
        .filter(|a| a.outcome == AttemptOutcome::Pending)
    {
        attempt.outcome = AttemptOutcome::Verified {
            actor: actor.clone(),
            at,
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests;
