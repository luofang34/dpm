use crate::EngineError;
use chrono::{DateTime, Utc};
use dpm_model::{
    ActorId, DependencyId, DependencyKind, DependencyPolicy, Endpoint, Plan, Release, Timeline,
    WorkItem, WorkItemId, WorkStatus,
};
use serde::{Deserialize, Serialize};

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
    },
    /// An open direct or inherited decision gates execution.
    Decision {
        /// Human-readable gate key.
        key: String,
    },
    /// The work is outside the active graph: not selected, undecided, awaiting an upstream choice,
    /// stranded behind excluded work, or an empty join.
    Applicability {
        /// Derived applicability explaining the exclusion.
        applicability: dpm_model::Applicability,
    },
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
    let applicability = timeline.applicability(work.id);
    if !applicability.is_applicable() {
        unmet.push(UnmetGate::Applicability {
            applicability: applicability.clone(),
        });
    }
    for dep in plan
        .enforced_dependencies()
        .filter(|dep| dep.successor == work.id && transition.governs(dep.kind))
    {
        let release = timeline.edge(plan, dep);
        if release.released_at().is_some() {
            continue;
        }
        if let Some(item) = plan.work_items.get(&dep.predecessor) {
            unmet.push(UnmetGate::Dependency {
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
            });
        }
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
            ..
        } => {
            let edge = format!("{} {lag_hours:+}h, {policy:?}", relation.abbreviation());
            let mut text = transition::describe_release(key, *requires, *release, &edge, *policy);
            if *lag_hours < 0.0 {
                text.push_str(&format!(
                    "; the {lag_hours}h lead shapes the schedule only and never releases work before the event"
                ));
            }
            text
        }
        UnmetGate::Decision { key } => format!("work awaits decision: {key}"),
        UnmetGate::Applicability { applicability } => {
            transition::describe_applicability(applicability)
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
