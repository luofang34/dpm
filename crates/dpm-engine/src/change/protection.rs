use super::invalid;
use crate::EngineError;
use dpm_model::{Plan, WorkItem, WorkItemId, WorkKind, WorkStatus};
use std::collections::BTreeSet;

pub(super) fn validate(current: &Plan, proposed: &Plan) -> Result<(), EngineError> {
    if current.revision != proposed.revision {
        return Err(EngineError::RevisionConflict {
            expected: proposed.revision,
            actual: current.revision,
        });
    }
    if current.workspace.id != proposed.workspace.id
        || current.format_version != proposed.format_version
    {
        return Err(invalid(
            current.workspace.id,
            "plan changes must preserve workspace identity and format",
        ));
    }
    if current.artifacts != proposed.artifacts {
        return Err(invalid(
            "artifacts",
            "use the evidence command; plan changes cannot alter evidence",
        ));
    }
    super::policy::protect_waivers(current, proposed)?;
    let locked = execution_basis(current);
    for work in current.work_items.values() {
        let next = proposed.work_items.get(&work.id);
        if locked.contains(&work.id) {
            if next != Some(work) {
                return Err(invalid(
                    &work.key,
                    "execution and its prerequisite contracts are protected; create follow-up work",
                ));
            }
            protect_context(current, proposed, work)?;
            let old_edges: Vec<_> = current
                .dependencies
                .iter()
                .filter(|d| d.successor == work.id)
                .collect();
            let new_edges: Vec<_> = proposed
                .dependencies
                .iter()
                .filter(|d| d.successor == work.id)
                .collect();
            if old_edges.len() != new_edges.len()
                || old_edges.iter().any(|d| !new_edges.contains(d))
            {
                return Err(invalid(
                    &work.key,
                    "cannot change the prerequisite basis of execution",
                ));
            }
        } else if next.is_none() && !work.artifact_ids.is_empty() {
            return Err(invalid(
                &work.key,
                "cannot delete work with attached evidence",
            ));
        } else if let Some(next) = next
            && (work.key != next.key || work.kind != next.kind || !same_execution(work, next))
        {
            return Err(invalid(
                &work.key,
                "preserve keys, kinds and execution fields; use lifecycle commands",
            ));
        }
    }
    validate_new_work(current, proposed)?;
    protect_observations(current, proposed)?;
    super::replacement::validate(current, proposed, &locked)
}

/// Observations are attributed reports like evidence and reviews, so review cannot author them.
fn protect_observations(current: &Plan, proposed: &Plan) -> Result<(), EngineError> {
    for (id, reference) in &proposed.external_references {
        let recorded = current
            .external_references
            .get(id)
            .and_then(|r| r.observation.as_ref());
        if reference.observation.as_ref() != recorded {
            return Err(invalid(
                id,
                "observations are recorded by link; plan changes cannot add or rewrite them",
            ));
        }
    }
    Ok(())
}

fn validate_new_work(current: &Plan, proposed: &Plan) -> Result<(), EngineError> {
    for work in proposed
        .work_items
        .values()
        .filter(|w| !current.work_items.contains_key(&w.id))
    {
        let expected = if work.kind == WorkKind::Task {
            WorkStatus::Proposed
        } else {
            WorkStatus::Planned
        };
        if work.status != expected
            || work.owner.is_some()
            || work.reported_progress_percent != 0
            || work.last_rejection.is_some()
            || work.block_reason.is_some()
            || !work.artifact_ids.is_empty()
        {
            return Err(invalid(
                &work.key,
                "new tasks must be Proposed; new work cannot carry execution or evidence",
            ));
        }
    }
    Ok(())
}

fn same_execution(a: &WorkItem, b: &WorkItem) -> bool {
    a.status == b.status
        && a.owner == b.owner
        && a.reported_progress_percent == b.reported_progress_percent
        && a.block_reason == b.block_reason
        && a.last_rejection == b.last_rejection
        && a.artifact_ids == b.artifact_ids
}

fn execution_basis(plan: &Plan) -> BTreeSet<WorkItemId> {
    let mut locked: BTreeSet<_> = plan
        .work_items
        .values()
        .filter(|w| {
            !matches!(w.status, WorkStatus::Proposed | WorkStatus::Planned)
                || w.owner.is_some()
                || w.last_rejection.is_some()
        })
        .map(|w| w.id)
        .collect();
    loop {
        let mut next = locked.clone();
        for id in &locked {
            next.extend(plan.work_items[id].parent);
            next.extend(
                plan.dependencies
                    .iter()
                    .filter(|d| d.successor == *id)
                    .map(|d| d.predecessor),
            );
        }
        if next == locked {
            return locked;
        }
        locked = next;
    }
}

fn protect_context(current: &Plan, proposed: &Plan, work: &WorkItem) -> Result<(), EngineError> {
    let mut project = Some(work.project);
    while let Some(id) = project {
        if current.projects.get(&id) != proposed.projects.get(&id) {
            return Err(invalid(
                &work.key,
                "cannot change the project basis of execution",
            ));
        }
        project = current.projects[&id].parent;
    }
    if work
        .requirement_ids
        .iter()
        .any(|id| current.requirements.get(id) != proposed.requirements.get(id))
        || work
            .resources
            .iter()
            .any(|r| current.resources.get(&r.resource) != proposed.resources.get(&r.resource))
    {
        return Err(invalid(
            &work.key,
            "cannot change the requirement or resource basis of execution",
        ));
    }
    Ok(())
}
