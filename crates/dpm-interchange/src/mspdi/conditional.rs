//! Conditional work on export: MSPDI has no decision options, conditions or branch joins, so the
//! document carries every task unconditionally and the report states what that loses.

use super::report::Finding;
use dpm_model::{Applicability, Dependency, JoinPolicy, Plan, WorkItem, WorkItemId};
use std::collections::BTreeMap;

/// Findings for a work item's own condition and join policy.
pub(super) fn findings(
    plan: &Plan,
    applicability: &BTreeMap<WorkItemId, Applicability>,
    work: &WorkItem,
) -> Vec<Finding> {
    let mut found = Vec::new();
    if let Some(condition) = &work.condition {
        let decision = plan
            .decisions
            .get(&condition.decision)
            .map_or_else(|| condition.decision.to_string(), |d| d.key.0.clone());
        found.push(Finding::new(
            "condition",
            format!(
                "applies only if {decision} selects {}; MSPDI has no conditional work, so it is written as unconditional",
                condition.option
            ),
        ));
    }
    if let JoinPolicy::ActiveBranches { allow_empty } = work.join {
        found.push(Finding::new(
            "join",
            format!(
                "active-branch join (allow_empty={allow_empty}) is not represented; MSPDI links always apply"
            ),
        ));
    }
    if let Some(state) = applicability.get(&work.id).filter(|a| !a.is_applicable()) {
        found.push(Finding::new(
            "applicability",
            format!(
                "currently outside the active graph ({state:?}) but written as schedulable work"
            ),
        ));
    }
    found
}

/// Note for a link whose predecessor a choice excluded; MSPDI would still enforce it.
pub(super) fn edge_note(
    applicability: &BTreeMap<WorkItemId, Applicability>,
    edge: &Dependency,
) -> Option<String> {
    applicability
        .get(&edge.predecessor)
        .filter(|a| a.is_not_selected())
        .map(|_| "predecessor is not selected; the link is written as enforced".into())
}
