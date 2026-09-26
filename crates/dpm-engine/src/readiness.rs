use crate::EngineError;
use dpm_model::{DecisionStatus, Plan, WorkItem, WorkItemId, WorkStatus};
pub use dpm_model::{completion, decisions_resolved};
use std::collections::BTreeSet;

/// Whether all predecessor work is complete under the MVP's conservative execution policy.
///
/// Temporal relationship kinds and lag affect schedule projections, not wall-clock execution timers.
/// Waived soft constraints are excluded; hard and unwaived soft constraints always apply.
pub fn dependencies_satisfied(plan: &Plan, work: WorkItemId) -> bool {
    dependencies_with_completion(plan, work, &completion(plan))
}

fn dependencies_with_completion(
    plan: &Plan,
    work: WorkItemId,
    done: &BTreeSet<WorkItemId>,
) -> bool {
    plan.work_items.contains_key(&work)
        && plan
            .enforced_dependencies()
            .filter(|dep| dep.successor == work)
            .all(|dep| done.contains(&dep.predecessor))
}

pub(crate) fn blocking_decision_keys(plan: &Plan, work: WorkItemId) -> Vec<String> {
    let Some(ids) = ancestors(plan, work) else {
        return Vec::new();
    };
    plan.decisions
        .values()
        .filter(|d| d.status == DecisionStatus::Open && !d.blocks.is_disjoint(&ids))
        .map(|d| d.key.to_string())
        .collect()
}

fn ancestors(plan: &Plan, work: WorkItemId) -> Option<BTreeSet<WorkItemId>> {
    let mut ids = BTreeSet::new();
    let mut next = Some(work);
    while let Some(id) = next {
        if !ids.insert(id) {
            return None;
        }
        next = plan.work_items.get(&id)?.parent;
    }
    Some(ids)
}

/// Whether a planned task with an execution contract can be claimed now.
pub fn is_ready(plan: &Plan, work: &WorkItem) -> bool {
    plan.work_items.get(&work.id) == Some(work)
        && crate::gate_report(plan, work.id).is_ok_and(|report| report.ready)
}

pub(crate) fn ready_with_completion(
    plan: &Plan,
    work: &WorkItem,
    done: &BTreeSet<WorkItemId>,
) -> bool {
    crate::gates::with_completion(plan, work, done).ready
}

/// Return a work projection with aggregate completion reflected in its lifecycle.
///
/// The returned value is a view, not an authoritative snapshot to persist.
pub fn show_work(plan: &Plan, work: WorkItemId) -> Result<WorkItem, EngineError> {
    plan.validate()?;
    let mut item = plan
        .work_items
        .get(&work)
        .ok_or(EngineError::MissingWorkItem(work))?
        .clone();
    if !item.is_executable() && completion(plan).contains(&work) {
        item.status = WorkStatus::Verified;
    }
    Ok(item)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
