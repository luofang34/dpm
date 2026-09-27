use super::nonempty;
use crate::EngineError;
use chrono::{DateTime, Utc};
use dpm_model::{
    ActorId, ActorKind, Dependency, DependencyId, DependencyPolicy, DependencyWaiver, Plan,
};

pub(super) fn waive(
    plan: &mut Plan,
    actor: &ActorId,
    dependency: DependencyId,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    let edge = authorized_edge(plan, actor, dependency, reason, Action::Waive)?;
    if edge.policy == DependencyPolicy::Hard {
        return Err(EngineError::InvalidCommand {
            entity: dependency.to_string(),
            reason: "hard constraints cannot be waived; change the policy through a reviewed plan change".into(),
        });
    }
    if edge.is_waived() {
        return Err(EngineError::InvalidCommand {
            entity: dependency.to_string(),
            reason: "dependency is already waived".into(),
        });
    }
    edge.waiver = Some(DependencyWaiver {
        actor: actor.clone(),
        at,
        reason: reason.into(),
    });
    Ok(())
}

/// Restoration re-imposes the constraint on later claims and verification; existing claims stay.
pub(super) fn restore(
    plan: &mut Plan,
    actor: &ActorId,
    dependency: DependencyId,
    reason: &str,
) -> Result<(), EngineError> {
    let edge = authorized_edge(plan, actor, dependency, reason, Action::Restore)?;
    if !edge.is_waived() {
        return Err(EngineError::InvalidCommand {
            entity: dependency.to_string(),
            reason: "dependency is not waived".into(),
        });
    }
    edge.waiver = None;
    Ok(())
}

#[derive(Clone, Copy)]
enum Action {
    Waive,
    Restore,
}

impl Action {
    fn describe(self) -> &'static str {
        match self {
            Self::Waive => "waive a dependency",
            Self::Restore => "restore a dependency",
        }
    }

    fn describe_own(self) -> &'static str {
        match self {
            Self::Waive => "waive a dependency of its own work",
            Self::Restore => "restore a dependency of its own work",
        }
    }
}

/// Setting a gate aside, or re-imposing it, is a judgement about both endpoints. As with basis
/// revalidation and relaxing plan changes, the owner of either task would otherwise relax a gate on
/// its own work, or on the result it hands to others, so the actor must not hold either task now or
/// have held it before a handoff. Verification differs: it performs the check rather than skipping
/// it, so only the verified task's holders are refused.
fn authorized_edge<'a>(
    plan: &'a mut Plan,
    actor: &ActorId,
    dependency: DependencyId,
    reason: &str,
    action: Action,
) -> Result<&'a mut Dependency, EngineError> {
    if actor.kind == ActorKind::Agent {
        return Err(EngineError::ActorNotAllowed {
            actor: actor.clone(),
            action: action.describe(),
        });
    }
    nonempty(&dependency.to_string(), "reason", reason)?;
    let edge = plan
        .find_dependency(dependency)
        .ok_or(EngineError::MissingDependency(dependency))?;
    let owns_endpoint = [edge.predecessor, edge.successor]
        .iter()
        .any(|id| plan.work_items.get(id).is_some_and(|w| w.held_by(actor)));
    if owns_endpoint {
        return Err(EngineError::ActorNotAllowed {
            actor: actor.clone(),
            action: action.describe_own(),
        });
    }
    plan.dependencies
        .iter_mut()
        .find(|edge| edge.id == dependency)
        .ok_or(EngineError::MissingDependency(dependency))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
