use crate::gates::{UnmetGate, evaluate};
use crate::{EngineError, Transition};
use chrono::{DateTime, Utc};
use dpm_model::{
    ActorId, Applicability, Dependency, DependencyId, DependencyKind, DependencyPolicy, Key, Plan,
    StartBasis, Timeline, WorkItemId, WorkKind,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// How a reviewed change weakens an edge that touches the applying actor's own work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeRelaxation {
    /// No edge between the same two work items remains, including when an endpoint is deleted.
    Removed,
    /// A Hard edge would become Soft, and so waivable.
    Policy,
    /// The lag would be lower.
    Lag,
    /// The relation would no longer imply the old one (FS implies SS and FF; each implies SF).
    Kind,
    /// A verified-finish start basis would accept a pending submission.
    StartBasis,
    /// Any other difference; only a strictly tighter or equal edge is accepted.
    Other,
}

/// A constraint that a reviewed change would relax for work the applying actor owns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelaxedConstraint {
    /// An edge into or out of the actor's work would be removed or weakened.
    Dependency {
        /// Identity of the current edge.
        dependency: DependencyId,
        /// Predecessor key.
        predecessor: Key,
        /// Successor key.
        successor: Key,
        /// How the edge would be weakened.
        relaxation: EdgeRelaxation,
    },
    /// A gate that is unmet now would no longer hold.
    Gate {
        /// Work whose transition the gate governs.
        work: Key,
        /// Governed transition.
        transition: Transition,
        /// The gate as `explain` reports it before the change.
        gate: UnmetGate,
    },
}

impl fmt::Display for RelaxedConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dependency {
                dependency,
                predecessor,
                successor,
                relaxation,
            } => write!(
                f,
                "dependency {dependency} ({predecessor} -> {successor}): {relaxation:?}"
            ),
            Self::Gate {
                work,
                transition,
                gate,
            } => write!(
                f,
                "the {transition} gate of {work}: {}",
                crate::gates::describe(gate)
            ),
        }
    }
}

/// Refuse a reviewed change that relaxes a gate on the actor's own work or on the result it
/// hands on.
///
/// The rule is the one waivers follow: an owner certifying that its own work, or the work waiting
/// for its result, needs less would be judging its own case. Edges are compared structurally, so a
/// relaxation counts even while the gate is closed for another reason; gates are compared through
/// the shared evaluator before and after, so relaxations through decisions, conditions or joins
/// count without being enumerated here.
pub(crate) fn refuse_own_relaxation(
    current: &Plan,
    proposed: &Plan,
    actor: &ActorId,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    let owned: BTreeSet<WorkItemId> = current
        .work_items
        .values()
        .filter(|w| w.held_by(actor))
        .map(|w| w.id)
        .collect();
    if owned.is_empty() {
        return Ok(());
    }
    let downstream = through_milestones(current, &owned, Direction::Downstream);
    let upstream = through_milestones(current, &owned, Direction::Upstream);
    let refuse = |work: Key, relaxed| EngineError::OwnGateRelaxed {
        actor: actor.clone(),
        work,
        relaxed: Box::new(relaxed),
    };
    for edge in current.dependencies.iter().filter(|e| {
        owned.contains(&e.predecessor)
            || owned.contains(&e.successor)
            || downstream.contains(&e.predecessor)
            || upstream.contains(&e.successor)
    }) {
        if let Some(relaxation) = edge_relaxation(edge, proposed) {
            let mine =
                if owned.contains(&edge.predecessor) || downstream.contains(&edge.predecessor) {
                    edge.predecessor
                } else {
                    edge.successor
                };
            let relaxed = RelaxedConstraint::Dependency {
                dependency: edge.id,
                predecessor: key(current, edge.predecessor),
                successor: key(current, edge.successor),
                relaxation,
            };
            return Err(refuse(key(current, mine), relaxed));
        }
    }
    // Gates downstream of a forwarding milestone wait on the owned result as if it were direct.
    let forwarding: BTreeSet<WorkItemId> = owned.union(&downstream).copied().collect();
    match relaxed_gate(current, proposed, &owned, &forwarding, at) {
        Some((mine, relaxed)) => Err(refuse(mine, relaxed)),
        None => Ok(()),
    }
}

/// Which way a milestone chain is followed from the owned work.
#[derive(Clone, Copy)]
enum Direction {
    /// Milestones forwarding the owned work's result to later work.
    Downstream,
    /// Milestones forwarding earlier results into the owned work.
    Upstream,
}

/// Milestones reached from owned work through milestone-only chains in one direction.
///
/// A milestone is a zero-duration point that forwards its prerequisites' results, so an edge out of
/// a downstream milestone carries the owned result on, and an edge into an upstream milestone gates
/// the owned work, exactly as if the edge touched the owned work directly. Other edges of those
/// milestones concern other work and stay outside the rule.
fn through_milestones(
    plan: &Plan,
    owned: &BTreeSet<WorkItemId>,
    direction: Direction,
) -> BTreeSet<WorkItemId> {
    let is_milestone = |id: &WorkItemId| {
        plan.work_items
            .get(id)
            .is_some_and(|w| w.kind == WorkKind::Milestone)
    };
    let mut reached = BTreeSet::new();
    let mut frontier: Vec<WorkItemId> = owned.iter().copied().collect();
    while let Some(node) = frontier.pop() {
        for edge in &plan.dependencies {
            let next = match direction {
                Direction::Downstream if edge.predecessor == node => edge.successor,
                Direction::Upstream if edge.successor == node => edge.predecessor,
                _ => continue,
            };
            if is_milestone(&next) && reached.insert(next) {
                frontier.push(next);
            }
        }
    }
    reached
}

/// The weakening of `old`, unless the proposal keeps an edge between the same endpoints that is
/// at least as strict (whatever its identity).
fn edge_relaxation(old: &Dependency, proposed: &Plan) -> Option<EdgeRelaxation> {
    let same_pair: Vec<_> = proposed
        .dependencies
        .iter()
        .filter(|e| e.predecessor == old.predecessor && e.successor == old.successor)
        .collect();
    let weakenings: Vec<_> = same_pair.iter().map(|e| weakening(old, e)).collect();
    if weakenings.iter().any(Option::is_none) {
        return None;
    }
    let nearest = same_pair
        .iter()
        .position(|e| e.id == old.id)
        .or((!same_pair.is_empty()).then_some(0));
    Some(
        nearest
            .and_then(|i| weakenings.get(i).copied().flatten())
            .unwrap_or(EdgeRelaxation::Removed),
    )
}

fn weakening(old: &Dependency, new: &Dependency) -> Option<EdgeRelaxation> {
    if old.policy == DependencyPolicy::Hard && new.policy == DependencyPolicy::Soft {
        return Some(EdgeRelaxation::Policy);
    }
    if new.lag_hours < old.lag_hours {
        return Some(EdgeRelaxation::Lag);
    }
    if !implies(new.kind, old.kind) {
        return Some(EdgeRelaxation::Kind);
    }
    if old.start_basis == StartBasis::Verified && new.start_basis == StartBasis::Provisional {
        return Some(EdgeRelaxation::StartBasis);
    }
    let normalized = Dependency {
        id: old.id,
        kind: old.kind,
        lag_hours: old.lag_hours,
        policy: old.policy,
        start_basis: old.start_basis,
        rationale: old.rationale.clone(),
        ..new.clone()
    };
    (normalized != *old).then_some(EdgeRelaxation::Other)
}

/// Whether relation `new` with the same lag bounds the successor at least as late as `old`: a
/// finish follows its start, so FS implies SS and FF, and each of those implies SF.
fn implies(new: DependencyKind, old: DependencyKind) -> bool {
    use DependencyKind::{FinishFinish, FinishStart, StartFinish, StartStart};
    new == old
        || new == FinishStart
        || (old == StartFinish && matches!(new, StartStart | FinishFinish))
}

/// A gate unmet now on owned work, or on a successor because of owned work, that the proposal
/// releases, with the key of the owned work it concerns.
fn relaxed_gate(
    current: &Plan,
    proposed: &Plan,
    owned: &BTreeSet<WorkItemId>,
    forwarding: &BTreeSet<WorkItemId>,
    at: DateTime<Utc>,
) -> Option<(Key, RelaxedConstraint)> {
    let (before, after) = (Timeline::at(current, at), Timeline::at(proposed, at));
    let forwarding_keys: BTreeSet<Key> = forwarding.iter().map(|id| key(current, *id)).collect();
    let successors = current
        .dependencies
        .iter()
        .filter(|e| forwarding.contains(&e.predecessor))
        .map(|e| e.successor);
    let scope: BTreeSet<_> = forwarding.iter().copied().chain(successors).collect();
    for id in scope {
        let (Some(old), Some(new)) = (current.work_items.get(&id), proposed.work_items.get(&id))
        else {
            continue;
        };
        for transition in Transition::ALL {
            let was = evaluate(current, old, transition, &before).unmet;
            let now = evaluate(proposed, new, transition, &after).unmet;
            for gate in was {
                let concerns = if owned.contains(&id) {
                    Some(old.key.clone())
                } else {
                    owned_subject(&gate, forwarding, &forwarding_keys)
                };
                if let Some(mine) = concerns
                    && !now.iter().any(|g| same_gate(g, &gate))
                {
                    let relaxed = RelaxedConstraint::Gate {
                        work: old.key.clone(),
                        transition,
                        gate,
                    };
                    return Some((mine, relaxed));
                }
            }
        }
    }
    None
}

/// The owned predecessor a successor's gate waits on, if any.
fn owned_subject(
    gate: &UnmetGate,
    owned: &BTreeSet<WorkItemId>,
    owned_keys: &BTreeSet<Key>,
) -> Option<Key> {
    match gate {
        UnmetGate::Dependency {
            predecessor, key, ..
        }
        | UnmetGate::BasisInvalidated {
            predecessor, key, ..
        } if owned.contains(predecessor) => Some(Key::new(key.clone())),
        UnmetGate::Applicability {
            applicability:
                Applicability::Stranded { predecessor, .. }
                | Applicability::AwaitingChoice { predecessor },
        } if owned_keys.contains(predecessor) => Some(predecessor.clone()),
        _ => None,
    }
}

/// Whether two unmet gates hold the same condition, whatever their current release detail.
fn same_gate(a: &UnmetGate, b: &UnmetGate) -> bool {
    match (a, b) {
        (
            UnmetGate::Dependency { predecessor: x, .. },
            UnmetGate::Dependency { predecessor: y, .. },
        ) => x == y,
        (
            UnmetGate::BasisInvalidated { dependency: x, .. },
            UnmetGate::BasisInvalidated { dependency: y, .. },
        ) => x == y,
        (UnmetGate::Decision { key: x }, UnmetGate::Decision { key: y })
        | (UnmetGate::Choice { key: x, .. }, UnmetGate::Choice { key: y, .. }) => x == y,
        _ => std::mem::discriminant(a) == std::mem::discriminant(b),
    }
}

fn key(plan: &Plan, id: WorkItemId) -> Key {
    plan.work_items
        .get(&id)
        .map_or_else(|| Key::new(id.to_string()), |w| w.key.clone())
}

#[cfg(test)]
mod tests;
