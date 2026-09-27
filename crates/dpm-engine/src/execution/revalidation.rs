//! Explicit review that re-bases started work after the predecessor attempt it relied on was
//! rejected. Nothing else records a new basis, so a later submission or verification of the
//! predecessor never validates the successor on its own.

use super::{nonempty, task_mut};
use crate::EngineError;
use chrono::{DateTime, Utc};
use dpm_model::{
    ActorId, ActorKind, BasisSource, BasisState, DependencyBasis, DependencyId, Plan, WorkItemId,
    basis_status,
};

/// Record the predecessor's current attempt as the successor's basis on one invalidated edge.
///
/// The reviewer must be a human or service, like any waiver: accepting work built on a rejected
/// result is an accountable judgement an agent cannot make for itself. No actor that owns or has
/// owned (before a handoff) the successor or the predecessor may review, because each would
/// certify its own work.
pub(super) fn revalidate(
    plan: &mut Plan,
    actor: &ActorId,
    work: WorkItemId,
    dependency: DependencyId,
    attempt: u32,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    nonempty(&work.to_string(), "revalidation reason", reason)?;
    if actor.kind == ActorKind::Agent {
        return Err(refused(actor, "revalidate a dependency basis"));
    }
    let item = task_mut(plan, work)?;
    if item.held_by(actor) {
        return Err(refused(actor, "revalidate the basis of its own work"));
    }
    let item = plan
        .work_items
        .get(&work)
        .ok_or(EngineError::MissingWorkItem(work))?;
    let status = basis_status(plan, item)
        .into_iter()
        .find(|s| s.dependency == dependency)
        .ok_or_else(|| EngineError::InvalidCommand {
            entity: dependency.to_string(),
            reason: "the work recorded no provisional basis on this dependency".into(),
        })?;
    if !matches!(status.state, BasisState::Invalidated { .. }) {
        return Err(EngineError::InvalidCommand {
            entity: dependency.to_string(),
            reason: format!(
                "the basis on {} attempt #{} was not rejected; only an invalidated basis is revalidated",
                status.predecessor_key, status.basis.attempt
            ),
        });
    }
    let predecessor = plan.work_items.get(&status.predecessor);
    if predecessor.is_some_and(|p| p.held_by(actor)) {
        return Err(refused(actor, "revalidate work against its own result"));
    }
    if status.current_attempt != Some(attempt) {
        return Err(EngineError::StaleAttempt {
            work,
            dependency,
            requested: attempt,
            current: status.current_attempt,
        });
    }
    task_mut(plan, work)?.basis.push(DependencyBasis {
        dependency,
        predecessor: status.predecessor,
        attempt,
        recorded_at: at,
        source: BasisSource::Revalidation {
            actor: actor.clone(),
            reason: reason.into(),
        },
    });
    Ok(())
}

fn refused(actor: &ActorId, action: &'static str) -> EngineError {
    EngineError::ActorNotAllowed {
        actor: actor.clone(),
        action,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
