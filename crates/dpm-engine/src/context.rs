use dpm_model::{
    Artifact, Decision, DecisionId, Dependency, Plan, Project, Requirement, Risk, WorkItem,
    WorkItemId, WorkLink,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Resolved domain records needed by an agent to execute a work contract without guessing references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// Assets named in this work contract, independent of local checkout paths.
    pub assets: Vec<dpm_model::WorkspaceAsset>,
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
    /// Incoming and outgoing temporal relationships, with identity, policy and any waiver.
    pub dependencies: Vec<Dependency>,
    /// Non-gating links in which this work is the source or target.
    pub links: Vec<WorkLink>,
    /// Directly dependent work items.
    pub successors: Vec<WorkItem>,
    /// External tracker objects linked to this work or its packages; never evidence or gates.
    #[serde(default)]
    pub external_references: Vec<dpm_model::ExternalReference>,
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

/// Whether a decision belongs to the context of work with these lineage IDs: it gates, relates
/// to, or decides the applicability of the work or a containing package.
pub(crate) fn decision_applies(
    plan: &Plan,
    decision: &Decision,
    lineage: &BTreeSet<WorkItemId>,
) -> bool {
    !decision.blocks.is_disjoint(lineage)
        || !decision.related_work.is_disjoint(lineage)
        || lineage.iter().any(|id| {
            plan.work_items
                .get(id)
                .and_then(|w| w.contract.condition.as_ref())
                .is_some_and(|c| c.decision == decision.id)
        })
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
        .filter(|d| decision_applies(plan, d, lineage))
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

pub(crate) fn execution_context(
    plan: &Plan,
    work: &WorkItem,
) -> Result<ExecutionContext, crate::EngineError> {
    let lineage = lineage(plan, work);
    let ancestors: BTreeSet<_> = lineage.iter().map(|w| w.id).collect();
    let parents: Vec<_> = lineage.iter().skip(1).map(|w| (*w).clone()).collect();
    let decisions: Vec<_> = applicable_decisions(plan, &ancestors)
        .into_iter()
        .filter_map(|id| plan.decisions.get(&id).cloned())
        .collect();
    let mut artifact_ids = work.execution.artifact_ids.clone();
    for decision in &decisions {
        artifact_ids.extend(&decision.artifact_ids);
    }
    for dependency in &plan.dependencies {
        if dependency.successor == work.id
            && let Some(predecessor) = plan.work_items.get(&dependency.predecessor)
        {
            artifact_ids.extend(&predecessor.execution.artifact_ids);
        }
    }
    let project = plan.projects.get(&work.project).cloned().ok_or_else(|| {
        crate::EngineError::InvalidCommand {
            entity: work.key.to_string(),
            reason: format!(
                "work names project {} the plan does not contain",
                work.project
            ),
        }
    })?;
    Ok(ExecutionContext {
        assets: work
            .contract
            .assets
            .iter()
            .filter_map(|r| plan.assets.get(&r.asset).cloned())
            .collect(),
        project,
        parents,
        requirements: work
            .contract
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
        links: plan
            .links
            .iter()
            .filter(|l| l.source == work.id || l.target == work.id)
            .cloned()
            .collect(),
        successors: plan
            .dependencies
            .iter()
            .filter(|d| d.predecessor == work.id)
            .filter_map(|d| plan.work_items.get(&d.successor).cloned())
            .collect(),
        external_references: plan
            .external_references
            .values()
            .filter(|r| r.links.iter().any(|l| ancestors.contains(&l.work)))
            .cloned()
            .collect(),
    })
}

#[cfg(test)]
mod tests;
