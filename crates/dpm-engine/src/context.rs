use dpm_model::{
    Artifact, Decision, DecisionId, Dependency, Plan, Project, Requirement, Risk, WorkItem,
    WorkItemId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Resolved domain records needed by an agent to execute a work contract without guessing references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// Resources named in this work contract, independent of local checkout paths.
    pub resources: Vec<dpm_model::Resource>,
    /// Project owning the selected work.
    pub project: Project,
    /// Work-package ancestors, nearest first.
    pub parents: Vec<WorkItem>,
    /// Requirements explicitly implemented by this work.
    pub requirements: Vec<Requirement>,
    /// Evidence attached to this work, its direct predecessors, or contextual decisions.
    pub artifacts: Vec<Artifact>,
    /// Direct or inherited gates and related decisions, including their reasons and sources.
    pub decisions: Vec<Decision>,
    /// Risks related to this work or its containing work packages.
    pub risks: Vec<Risk>,
    /// Incoming and outgoing temporal relationships.
    pub dependencies: Vec<Dependency>,
    /// Directly dependent work items.
    pub successors: Vec<WorkItem>,
}

/// Work and its containing packages, nearest first; decisions and risks attach through any of them.
pub(crate) fn lineage<'a>(plan: &'a Plan, work: &'a WorkItem) -> Vec<&'a WorkItem> {
    let mut items = vec![work];
    let mut parent = work.parent;
    while let Some(item) = parent.and_then(|id| plan.work_items.get(&id)) {
        items.push(item);
        parent = item.parent;
    }
    items
}

/// Whether a decision belongs to the context of work with these lineage IDs.
pub(crate) fn decision_applies(decision: &Decision, lineage: &BTreeSet<WorkItemId>) -> bool {
    !decision.blocks.is_disjoint(lineage) || !decision.related_work.is_disjoint(lineage)
}

/// Decisions linked to the lineage plus every replacement reachable through `supersedes`.
///
/// A replacement need not repeat the old decision's work links, so work that was linked only to a
/// superseded choice would otherwise lose sight of the choice that now applies to it.
pub(crate) fn applicable_decisions(
    plan: &Plan,
    lineage: &BTreeSet<WorkItemId>,
) -> BTreeSet<DecisionId> {
    let mut selected: BTreeSet<_> = plan
        .decisions
        .values()
        .filter(|d| decision_applies(d, lineage))
        .map(|d| d.id)
        .collect();
    loop {
        let before = selected.len();
        let replacements: Vec<_> = plan
            .decisions
            .values()
            .filter(|d| d.supersedes.is_some_and(|old| selected.contains(&old)))
            .map(|d| d.id)
            .collect();
        selected.extend(replacements);
        if selected.len() == before {
            return selected;
        }
    }
}

pub(crate) fn execution_context(plan: &Plan, work: &WorkItem) -> ExecutionContext {
    let lineage = lineage(plan, work);
    let ancestors: BTreeSet<_> = lineage.iter().map(|w| w.id).collect();
    let parents: Vec<_> = lineage.iter().skip(1).map(|w| (*w).clone()).collect();
    let decisions: Vec<_> = applicable_decisions(plan, &ancestors)
        .into_iter()
        .map(|id| plan.decisions[&id].clone())
        .collect();
    let mut artifact_ids = work.artifact_ids.clone();
    for decision in &decisions {
        artifact_ids.extend(&decision.artifact_ids);
    }
    for dependency in &plan.dependencies {
        if dependency.successor == work.id {
            artifact_ids.extend(&plan.work_items[&dependency.predecessor].artifact_ids);
        }
    }
    ExecutionContext {
        resources: work
            .resources
            .iter()
            .filter_map(|r| plan.resources.get(&r.resource).cloned())
            .collect(),
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
        decisions,
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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests;
