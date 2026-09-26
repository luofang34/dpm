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
    let edge = authorized_edge(plan, actor, dependency, reason, "waive a dependency")?;
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
    let edge = authorized_edge(plan, actor, dependency, reason, "restore a dependency")?;
    if !edge.is_waived() {
        return Err(EngineError::InvalidCommand {
            entity: dependency.to_string(),
            reason: "dependency is not waived".into(),
        });
    }
    edge.waiver = None;
    Ok(())
}

fn authorized_edge<'a>(
    plan: &'a mut Plan,
    actor: &ActorId,
    dependency: DependencyId,
    reason: &str,
    action: &'static str,
) -> Result<&'a mut Dependency, EngineError> {
    if actor.kind == ActorKind::Agent {
        return Err(EngineError::ActorNotAllowed {
            actor: actor.clone(),
            action,
        });
    }
    nonempty(&dependency.to_string(), "reason", reason)?;
    plan.dependencies
        .iter_mut()
        .find(|edge| edge.id == dependency)
        .ok_or(EngineError::MissingDependency(dependency))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
