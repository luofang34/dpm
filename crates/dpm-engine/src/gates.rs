use crate::{EngineError, completion};
use dpm_model::{ActorId, DependencyKind, Plan, WorkItem, WorkItemId, WorkStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Machine-readable reason preventing a claim under the verified-predecessor execution policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UnmetGate {
    /// Containers and milestones are projections, not claimable work.
    Aggregate,
    /// Proposed work needs explicit approval of its execution contract.
    ContractNotRatified,
    /// An existing lifecycle state prevents a new claim.
    Lifecycle {
        /// Current authoritative lifecycle.
        status: WorkStatus,
    },
    /// A worker recorded a blocker.
    Blocker {
        /// Concrete blocker text.
        reason: String,
    },
    /// A predecessor must complete independently of the temporal projection.
    Dependency {
        /// Stable identity of the prerequisite.
        predecessor: WorkItemId,
        /// Human-readable prerequisite key.
        key: String,
        /// Project containing the prerequisite.
        project: String,
        /// Resources required by the prerequisite; these are context, not satisfied gates.
        resource_keys: Vec<String>,
        /// Schedule relation; all relations currently require verified completion for execution.
        relation: DependencyKind,
        /// Lead/lag in the schedule projection, not an execution timer.
        lag_hours: f64,
        /// Authoritative state of the incomplete prerequisite.
        status: WorkStatus,
        /// Worker responsible for the prerequisite, if reserved.
        owner: Option<ActorId>,
    },
    /// An open direct or inherited decision gates execution.
    Decision {
        /// Human-readable gate key.
        key: String,
    },
}

/// Shared claim eligibility for queries, commands and terminal projections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateReport {
    /// Whether this leaf task can be claimed now.
    pub ready: bool,
    /// Every unmet condition; an empty list means ready.
    pub unmet: Vec<UnmetGate>,
}

/// Explain claim eligibility without changing authoritative state.
pub fn gate_report(plan: &Plan, work: WorkItemId) -> Result<GateReport, EngineError> {
    plan.validate()?;
    let item = plan
        .work_items
        .get(&work)
        .ok_or(EngineError::MissingWorkItem(work))?;
    Ok(with_completion(plan, item, &completion(plan)))
}

pub(crate) fn with_completion(
    plan: &Plan,
    work: &WorkItem,
    done: &BTreeSet<WorkItemId>,
) -> GateReport {
    let mut unmet = Vec::new();
    if !work.is_executable() {
        unmet.push(UnmetGate::Aggregate);
    }
    match work.status {
        WorkStatus::Proposed => unmet.push(UnmetGate::ContractNotRatified),
        WorkStatus::Planned => {}
        status => unmet.push(UnmetGate::Lifecycle { status }),
    }
    if let Some(reason) = &work.block_reason {
        unmet.push(UnmetGate::Blocker {
            reason: reason.clone(),
        });
    }
    for dep in plan
        .dependencies
        .iter()
        .filter(|dep| dep.successor == work.id && !done.contains(&dep.predecessor))
    {
        if let Some(item) = plan.work_items.get(&dep.predecessor) {
            unmet.push(UnmetGate::Dependency {
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
                status: item.status,
                owner: item.owner.clone(),
            });
        }
    }
    unmet.extend(
        crate::readiness::blocking_decision_keys(plan, work.id)
            .into_iter()
            .map(|key| UnmetGate::Decision { key }),
    );
    GateReport {
        ready: unmet.is_empty(),
        unmet,
    }
}

impl GateReport {
    /// Human-readable rendering of the same structured conditions returned to agents.
    pub fn reasons(&self) -> Vec<String> {
        self.unmet.iter().map(|gate| match gate {
            UnmetGate::Aggregate => "aggregate work completes through its prerequisites or children".into(),
            UnmetGate::ContractNotRatified => "contract not ratified".into(),
            UnmetGate::Lifecycle { status } => format!("work lifecycle state is {status:?}"),
            UnmetGate::Blocker { reason } => format!("work is blocked: {reason}"),
            UnmetGate::Dependency { key, relation, lag_hours, .. } => format!("awaits verified prerequisite {key} ({relation:?}, {lag_hours:+}h; relation and lag apply to schedule projection)"),
            UnmetGate::Decision { key } => format!("work awaits decision: {key}"),
        }).collect()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
