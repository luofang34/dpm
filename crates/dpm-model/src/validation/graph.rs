use super::{ValidationError, invalid};
use crate::{Plan, WorkKind};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(super) fn validate(plan: &Plan) -> Result<(), ValidationError> {
    project_hierarchy(plan)?;
    work_hierarchy(plan)?;
    execution_graph(plan)
}

fn project_hierarchy(plan: &Plan) -> Result<(), ValidationError> {
    for project in plan.projects.values() {
        let mut seen = BTreeSet::from([project.id]);
        let mut parent = project.parent;
        while let Some(id) = parent {
            if !seen.insert(id) {
                return Err(invalid("project", project.id, "containment cycle"));
            }
            parent = plan
                .projects
                .get(&id)
                .ok_or_else(|| invalid("project", project.id, format!("missing parent {id}")))?
                .parent;
        }
    }
    Ok(())
}

fn work_hierarchy(plan: &Plan) -> Result<(), ValidationError> {
    for work in plan.work_items.values() {
        let mut seen = BTreeSet::from([work.id]);
        let mut parent = work.parent;
        while let Some(id) = parent {
            if !seen.insert(id) {
                return Err(invalid("work", work.id, "containment cycle"));
            }
            let container = plan
                .work_items
                .get(&id)
                .ok_or_else(|| invalid("work", work.id, format!("missing parent {id}")))?;
            if container.kind != WorkKind::WorkPackage || container.project != work.project {
                return Err(invalid(
                    "work",
                    work.id,
                    "parent must be a work package in the same project",
                ));
            }
            parent = container.parent;
        }
    }
    Ok(())
}

fn execution_graph(plan: &Plan) -> Result<(), ValidationError> {
    let mut edges = BTreeSet::new();
    for dep in &plan.dependencies {
        for id in [dep.predecessor, dep.successor] {
            let item = plan.work_items.get(&id).ok_or_else(|| {
                invalid("dependency", dep.successor, format!("missing work {id}"))
            })?;
            if item.kind == WorkKind::WorkPackage {
                return Err(invalid(
                    "dependency",
                    id,
                    "use a task or milestone endpoint, not a work package",
                ));
            }
        }
        if !dep.lag_hours.is_finite() {
            return Err(invalid("dependency", dep.successor, "lag must be finite"));
        }
        edges.insert((dep.predecessor, dep.successor));
    }
    let mut indegree: BTreeMap<_, usize> = plan.work_items.keys().map(|id| (*id, 0)).collect();
    let mut outgoing: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for (from, to) in edges {
        outgoing.entry(from).or_default().push(to);
        let count = indegree.entry(to).or_default();
        *count = count.wrapping_add(1);
    }
    let mut queue: VecDeque<_> = indegree
        .iter()
        .filter_map(|(id, n)| (*n == 0).then_some(*id))
        .collect();
    let mut visited: usize = 0;
    while let Some(id) = queue.pop_front() {
        visited = visited.wrapping_add(1);
        for child in outgoing.get(&id).into_iter().flatten() {
            if let Some(count) = indegree.get_mut(child) {
                *count -= 1;
                if *count == 0 {
                    queue.push_back(*child);
                }
            }
        }
    }
    if visited != plan.work_items.len() {
        return Err(invalid(
            "dependencies",
            plan.workspace.id,
            "dependency graph contains a cycle",
        ));
    }
    Ok(())
}
