use crate::EngineError;
use dpm_model::Plan;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

mod applicability;
pub use applicability::ApplicabilityChange;
mod independence;
pub(crate) use independence::refuse_own_relaxation;
pub use independence::{EdgeRelaxation, RelaxedConstraint};
mod policy;
mod protection;
mod replacement;
pub use replacement::AffectedWork;

/// One entity-level semantic difference, including explicit additions and removals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityChange {
    /// Domain collection such as `external_references`, `dependencies` per edge, or the workspace-wide
    /// `workspace` / `links`.
    pub collection: String,
    /// Stable entity identity, absent for workspace-wide values.
    pub id: Option<String>,
    /// Changed field names; empty for an added or removed entity.
    pub fields: Vec<String>,
    /// Full previous entity; null means addition.
    pub before: Value,
    /// Full proposed entity; null means deletion.
    pub after: Value,
}

/// Validated, read-only review of a proposed plan against one revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangePreview {
    /// Revision that must still be current when applying the proposal.
    pub base_revision: u64,
    /// Deterministically ordered semantic differences.
    pub changes: Vec<EntityChange>,
    /// Work to reassess because a decision in its context is replaced; never a gate.
    pub affected_work: Vec<AffectedWork>,
    /// Work entering or leaving the active graph, including in-flight work a choice change affects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applicability_changes: Vec<ApplicabilityChange>,
}

/// Validate a proposed plan without changing state, lifecycle, evidence or decision outcomes.
///
/// New tasks must be Proposed. Existing execution and its prerequisite/context basis are protected;
/// review is a prerequisite to applying a change, not permission for agents to approve scope.
pub fn propose_change(current: &Plan, proposed: &Plan) -> Result<ChangePreview, EngineError> {
    current.validate()?;
    proposed.validate()?;
    protection::validate(current, proposed)?;
    let before = serde_json::to_value(current)?;
    let after = serde_json::to_value(proposed)?;
    let mut changes = Vec::new();
    append(
        &mut changes,
        "workspace",
        None,
        &before["workspace"],
        &after["workspace"],
    );
    for collection in [
        "projects",
        "resources",
        "work_items",
        "requirements",
        "decisions",
        "risks",
        "external_references",
    ] {
        let ids: BTreeSet<_> = current_keys(&before[collection])
            .chain(current_keys(&after[collection]))
            .collect();
        for id in ids {
            append(
                &mut changes,
                collection,
                Some(id.clone()),
                &before[collection][&id],
                &after[collection][&id],
            );
        }
    }
    let before_edges = edges_by_id(current)?;
    let after_edges = edges_by_id(proposed)?;
    for id in before_edges
        .keys()
        .chain(after_edges.keys())
        .collect::<BTreeSet<_>>()
    {
        append(
            &mut changes,
            "dependencies",
            Some(id.clone()),
            before_edges.get(id).unwrap_or(&Value::Null),
            after_edges.get(id).unwrap_or(&Value::Null),
        );
    }
    append(
        &mut changes,
        "links",
        None,
        &sorted_links(current)?,
        &sorted_links(proposed)?,
    );
    Ok(ChangePreview {
        base_revision: current.revision,
        changes,
        affected_work: replacement::affected_work(current, proposed),
        applicability_changes: applicability::changes(current, proposed),
    })
}

fn current_keys(value: &Value) -> impl Iterator<Item = String> + '_ {
    value
        .as_object()
        .into_iter()
        .flat_map(|map| map.keys().cloned())
}

fn append(
    changes: &mut Vec<EntityChange>,
    collection: &str,
    id: Option<String>,
    before: &Value,
    after: &Value,
) {
    if before == after {
        return;
    }
    let fields = if before.is_object() && after.is_object() {
        current_keys(before)
            .chain(current_keys(after))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|field| before[field] != after[field])
            .collect()
    } else {
        Vec::new()
    };
    changes.push(EntityChange {
        collection: collection.into(),
        id,
        fields,
        before: before.clone(),
        after: after.clone(),
    });
}

fn edges_by_id(plan: &Plan) -> Result<BTreeMap<String, Value>, EngineError> {
    plan.dependencies
        .iter()
        .map(|edge| Ok((edge.id.to_string(), serde_json::to_value(edge)?)))
        .collect()
}

fn sorted_links(plan: &Plan) -> Result<Value, EngineError> {
    let mut links = plan.links.clone();
    links.sort_by_key(|l| (l.kind, l.source, l.target));
    Ok(serde_json::to_value(links)?)
}

pub(crate) fn invalid(entity: impl ToString, reason: &str) -> EngineError {
    EngineError::InvalidCommand {
        entity: entity.to_string(),
        reason: reason.into(),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
