use crate::{DecisionStatus, Plan, WorkItemId, WorkKind, WorkStatus};
use std::collections::BTreeSet;

/// Derive completed task, milestone, and work-package identities without changing stored state.
///
/// Milestones need at least one unwaived prerequisite and work packages need at least one child.
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
                    .enforced_dependencies()
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

/// Whether gates on this work and its containing work packages are resolved.
pub fn decisions_resolved(plan: &Plan, work: WorkItemId) -> bool {
    ancestors(plan, work).is_some_and(|ids| {
        !plan
            .decisions
            .values()
            .any(|d| d.status == DecisionStatus::Open && !d.blocks.is_disjoint(&ids))
    })
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
