use crate::EngineError;
use dpm_model::{DecisionStatus, Plan, WorkItem, WorkItemId, WorkKind, WorkStatus};
use std::collections::BTreeSet;

/// Derive completed task, milestone, and work-package identities without changing stored state.
///
/// Milestones need at least one prerequisite and work packages need at least one child.
pub fn completion(plan: &Plan) -> BTreeSet<WorkItemId> {
    let mut done: BTreeSet<_> = plan
        .work_items
        .values()
        .filter(|w| w.is_executable() && w.status.satisfies_dependency())
        .map(|w| w.id)
        .collect();
    loop {
        let previous = done.len();
        for work in plan.work_items.values() {
            if done.contains(&work.id)
                || work.status != WorkStatus::Planned
                || !decisions_resolved(plan, work.id)
            {
                continue;
            }
            let prerequisites: Vec<_> = match work.kind {
                WorkKind::Task => continue,
                WorkKind::Milestone => plan
                    .dependencies
                    .iter()
                    .filter(|d| d.successor == work.id)
                    .map(|d| d.predecessor)
                    .collect(),
                WorkKind::WorkPackage => plan
                    .work_items
                    .values()
                    .filter(|w| w.parent == Some(work.id))
                    .map(|w| w.id)
                    .collect(),
            };
            if !prerequisites.is_empty() && prerequisites.iter().all(|id| done.contains(id)) {
                done.insert(work.id);
            }
        }
        if previous == done.len() {
            return done;
        }
    }
}

/// Whether all predecessor work is complete under the MVP's conservative execution policy.
///
/// Temporal relationship kinds and lag affect schedule projections, not wall-clock execution timers.
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
            .dependencies
            .iter()
            .filter(|dep| dep.successor == work)
            .all(|dep| done.contains(&dep.predecessor))
}

/// Whether gates on this work and its containing work packages are resolved.
pub fn decisions_resolved(plan: &Plan, work: WorkItemId) -> bool {
    ancestors(plan, work).is_some_and(|ids| {
        !plan
            .decisions
            .values()
            .any(|d| d.status == DecisionStatus::Open && !d.blocks.is_disjoint(&ids))
    })
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
