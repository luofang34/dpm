use crate::EngineError;
use dpm_model::Plan;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

mod protection;

/// One entity-level semantic difference, including explicit additions and removals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityChange {
    /// Domain collection, or `workspace` / `dependencies`.
    pub collection: String,
    /// Stable entity identity, absent for workspace-wide constraints.
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
    append(
        &mut changes,
        "dependencies",
        None,
        &sorted_dependencies(current)?,
        &sorted_dependencies(proposed)?,
    );
    Ok(ChangePreview {
        base_revision: current.revision,
        changes,
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

fn sorted_dependencies(plan: &Plan) -> Result<Value, EngineError> {
    let mut edges = plan.dependencies.clone();
    edges.sort_by(|a, b| {
        (a.predecessor, a.successor, a.kind as u8)
            .cmp(&(b.predecessor, b.successor, b.kind as u8))
            .then(a.lag_hours.total_cmp(&b.lag_hours))
    });
    Ok(serde_json::to_value(edges)?)
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
