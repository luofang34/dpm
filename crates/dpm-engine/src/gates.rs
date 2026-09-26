use crate::EngineError;
use chrono::{DateTime, Utc};
use dpm_model::{
    ActorId, BasisState, Dependency, DependencyId, DependencyKind, DependencyPolicy, Endpoint,
    Plan, Release, StartBasis, StartRelease, Timeline, WorkItem, WorkItemId, WorkStatus,
};
use serde::{Deserialize, Serialize};

mod provisional;
mod transition;
pub use transition::Transition;

/// Machine-readable reason preventing a lifecycle transition at the evaluated time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UnmetGate {
    /// Containers and milestones are projections, not claimable work.
    Aggregate,
    /// Proposed work needs explicit approval of its execution contract.
    ContractNotRatified,
    /// The current lifecycle state does not permit this transition.
    Lifecycle {
        /// Current authoritative lifecycle.
        status: WorkStatus,
    },
    /// A worker recorded a blocker.
    Blocker {
        /// Concrete blocker text.
        reason: String,
    },
    /// An unwaived constraint governing this transition is not yet released.
    Dependency {
        /// Stable identity of the unwaived constraint.
        dependency: DependencyId,
        /// Whether a human or service may waive the constraint.
        policy: DependencyPolicy,
        /// Stable identity of the prerequisite.
        predecessor: WorkItemId,
        /// Human-readable prerequisite key.
        key: String,
        /// Project containing the prerequisite.
        project: String,
        /// Resources required by the prerequisite; these are context, not satisfied gates.
        resource_keys: Vec<String>,
        /// Relation; FS and SS govern the successor's start, FF and SF its finish.
        relation: DependencyKind,
        /// Positive lag must elapse after the event; negative lag shapes the schedule only.
        lag_hours: f64,
        /// Predecessor event the relation waits for: start, or finish (verification).
        requires: Endpoint,
        /// Why the constraint is not released: missing event, elapsing lag or unknown event time.
        release: Release,
        /// Authoritative state of the prerequisite.
        status: WorkStatus,
        /// Worker responsible for the prerequisite, if reserved.
        owner: Option<ActorId>,
        /// Predecessor result the edge accepts for the successor's start.
        #[serde(default, skip_serializing_if = "StartBasis::is_verified")]
        start_basis: StartBasis,
        /// Whether this evaluation accepts a pending submission; false for verification.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        accepts_submission: bool,
        /// Pending predecessor attempt whose positive lag is still elapsing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<u32>,
    },
    /// The execution relies on a predecessor attempt that was rejected; finishing waits for an
    /// explicit independent revalidation against a newer attempt.
    BasisInvalidated {
        /// Provisional edge carrying the basis.
        dependency: DependencyId,
        /// Stable identity of the predecessor.
        predecessor: WorkItemId,
        /// Human-readable predecessor key.
        key: String,
        /// Ordinal of the rejected attempt the execution relies on.
        attempt: u32,
        /// Rejection of that attempt.
        state: BasisState,
        /// Predecessor's pending or verified attempt a revalidation may name, if any.
        current_attempt: Option<u32>,
    },
    /// An open direct or inherited decision gates execution.
    Decision {
        /// Human-readable gate key.
        key: String,
    },
}

/// A provisional edge released on a predecessor's pending attempt; a start records it as basis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProvisionalRelease {
    /// Provisional edge.
    pub dependency: DependencyId,
    /// Predecessor that made the attempt.
    pub predecessor: WorkItemId,
    /// Human-readable predecessor key.
    pub key: String,
    /// Ordinal of the pending attempt.
    pub attempt: u32,
}

/// Shared eligibility of one lifecycle transition for queries, commands and terminal projections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateReport {
    /// Transition this report evaluates; `claim` is the readiness used by `next`.
    #[serde(default)]
    pub transition: Transition,
    /// Whether the transition is permitted at the evaluated time.
    pub ready: bool,
    /// Every unmet condition; an empty list means ready.
    pub unmet: Vec<UnmetGate>,
    /// Start-gate edges released only by a pending predecessor attempt, not a verified finish.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provisional: Vec<ProvisionalRelease>,
}

/// Explain whether a transition is permitted at an adapter-supplied time, without changing state.
pub fn gate_report(
    plan: &Plan,
    work: WorkItemId,
    transition: Transition,
    now: DateTime<Utc>,
) -> Result<GateReport, EngineError> {
    plan.validate()?;
    let item = plan
        .work_items
        .get(&work)
        .ok_or(EngineError::MissingWorkItem(work))?;
    Ok(evaluate(plan, item, transition, &Timeline::at(plan, now)))
}

/// The one gate evaluation used by every query, command and view.
pub(crate) fn evaluate(
    plan: &Plan,
    work: &WorkItem,
    transition: Transition,
    timeline: &Timeline,
) -> GateReport {
    let mut unmet = Vec::new();
    if !work.is_executable() {
        unmet.push(UnmetGate::Aggregate);
    }
    match work.status {
        status if status == transition.lifecycle() => {}
        WorkStatus::Proposed => unmet.push(UnmetGate::ContractNotRatified),
        status => unmet.push(UnmetGate::Lifecycle { status }),
    }
    if let Some(reason) = &work.block_reason {
        unmet.push(UnmetGate::Blocker {
            reason: reason.clone(),
        });
    }
    let mut provisional = Vec::new();
    for dep in plan
        .enforced_dependencies()
        .filter(|dep| dep.successor == work.id && transition.governs(dep.kind))
    {
        let Some(item) = plan.work_items.get(&dep.predecessor) else {
            continue;
        };
        let edge = if transition.starts() {
            timeline.start_edge(plan, dep)
        } else {
            StartRelease {
                release: timeline.edge(plan, dep),
                attempt: None,
            }
        };
        match (edge.release.released_at(), edge.attempt) {
            (Some(_), None) => {}
            (Some(_), Some(attempt)) => provisional.push(ProvisionalRelease {
                dependency: dep.id,
                predecessor: item.id,
                key: item.key.to_string(),
                attempt,
            }),
            (None, attempt) => unmet.push(dependency_gate(
                plan,
                dep,
                item,
                transition,
                edge.release,
                attempt,
            )),
        }
    }
    if transition.checks_basis() {
        unmet.extend(provisional::basis_gates(plan, work));
    }
    if transition.checks_decisions() {
        unmet.extend(
            timeline
                .decisions(plan, work.id)
                .into_iter()
                .filter(|(_, release)| release.released_at().is_none())
                .map(|(decision, _)| UnmetGate::Decision {
                    key: decision.key.to_string(),
                }),
        );
    }
    GateReport {
        transition,
        ready: unmet.is_empty(),
        unmet,
        provisional,
    }
}

fn dependency_gate(
    plan: &Plan,
    dep: &Dependency,
    item: &WorkItem,
    transition: Transition,
    release: Release,
    attempt: Option<u32>,
) -> UnmetGate {
    UnmetGate::Dependency {
        dependency: dep.id,
        policy: dep.policy,
        resource_keys: item
            .resources
            .iter()
            .filter_map(|r| plan.resources.get(&r.resource))
            .map(|r| r.key.to_string())
            .collect(),
        predecessor: item.id,
        key: item.key.to_string(),
        project: plan
            .projects
            .get(&item.project)
            .map(|p| p.key.to_string())
            .unwrap_or_default(),
        relation: dep.kind,
        lag_hours: dep.lag_hours,
        requires: dep.kind.predecessor_endpoint(),
        release,
        status: item.status,
        owner: item.owner.clone(),
        start_basis: dep.start_basis,
        accepts_submission: transition.starts() && dep.start_basis == StartBasis::Provisional,
        attempt,
    }
}

impl GateReport {
    /// Human-readable rendering of the same structured conditions returned to agents.
    pub fn reasons(&self) -> Vec<String> {
        self.unmet.iter().map(describe).collect()
    }
}

pub(crate) fn summary(unmet: &[UnmetGate]) -> String {
    unmet.iter().map(describe).collect::<Vec<_>>().join("; ")
}

/// Human-readable rendering of one unmet condition.
pub(crate) fn describe(gate: &UnmetGate) -> String {
    match gate {
        UnmetGate::Aggregate => {
            "aggregate work completes through its prerequisites or children".into()
        }
        UnmetGate::ContractNotRatified => "contract not ratified".into(),
        UnmetGate::Lifecycle { status } => format!("work lifecycle state is {status:?}"),
        UnmetGate::Blocker { reason } => format!("work is blocked: {reason}"),
        UnmetGate::Dependency {
            key,
            relation,
            lag_hours,
            policy,
            requires,
            release,
            start_basis,
            accepts_submission,
            attempt,
            ..
        } => {
            let edge = format!("{} {lag_hours:+}h, {policy:?}", relation.abbreviation());
            let mut text = if *accepts_submission {
                provisional::describe_start(key, *release, *attempt, &edge, *policy)
            } else {
                transition::describe_release(key, *requires, *release, &edge, *policy)
            };
            if *start_basis == StartBasis::Provisional && !*accepts_submission {
                text.push_str("; a provisional edge releases only the start, never finishing on an unverified result");
            }
            if *lag_hours < 0.0 {
                text.push_str(&format!(
                    "; the {lag_hours}h lead shapes the schedule only and never releases work before the event"
                ));
            }
            text
        }
        UnmetGate::BasisInvalidated {
            key,
            attempt,
            state,
            current_attempt,
            ..
        } => provisional::describe_invalidated(key, *attempt, state, *current_attempt),
        UnmetGate::Decision { key } => format!("work awaits decision: {key}"),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
