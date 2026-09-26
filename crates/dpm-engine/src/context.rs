use dpm_model::{Artifact, Decision, Dependency, Plan, Project, Requirement, Risk, WorkItem};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Resolved domain records needed by an agent to execute a work contract without guessing references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// Project owning the selected work.
    pub project: Project,
    /// Work-package ancestors, nearest first.
    pub parents: Vec<WorkItem>,
    /// Requirements explicitly implemented by this work.
    pub requirements: Vec<Requirement>,
    /// Evidence attached to this work or its direct predecessors.
    pub artifacts: Vec<Artifact>,
    /// Direct or inherited decision gates, including their resolution.
    pub decisions: Vec<Decision>,
    /// Risks related to this work or its containing work packages.
    pub risks: Vec<Risk>,
    /// Incoming and outgoing temporal relationships.
    pub dependencies: Vec<Dependency>,
    /// Directly dependent work items.
    pub successors: Vec<WorkItem>,
}

pub(crate) fn execution_context(plan: &Plan, work: &WorkItem) -> ExecutionContext {
    let mut ancestors = BTreeSet::from([work.id]);
    let mut parents = Vec::new();
    let mut parent = work.parent;
    while let Some(item) = parent.and_then(|id| plan.work_items.get(&id)) {
        ancestors.insert(item.id);
        parents.push(item.clone());
        parent = item.parent;
    }
    let mut artifact_ids = work.artifact_ids.clone();
    for dependency in &plan.dependencies {
        if dependency.successor == work.id {
            artifact_ids.extend(&plan.work_items[&dependency.predecessor].artifact_ids);
        }
    }
    ExecutionContext {
        project: plan.projects[&work.project].clone(),
        parents,
        requirements: work
            .requirement_ids
            .iter()
            .filter_map(|id| plan.requirements.get(id).cloned())
            .collect(),
        artifacts: artifact_ids
            .iter()
            .filter_map(|id| plan.artifacts.get(id).cloned())
            .collect(),
        decisions: plan
            .decisions
            .values()
            .filter(|d| !d.blocks.is_disjoint(&ancestors))
            .cloned()
            .collect(),
        risks: plan
            .risks
            .values()
            .filter(|r| !r.related_work.is_disjoint(&ancestors))
            .cloned()
            .collect(),
        dependencies: plan
            .dependencies
            .iter()
            .filter(|d| d.predecessor == work.id || d.successor == work.id)
            .cloned()
            .collect(),
        successors: plan
            .dependencies
            .iter()
            .filter(|d| d.predecessor == work.id)
            .filter_map(|d| plan.work_items.get(&d.successor).cloned())
            .collect(),
    }
}
