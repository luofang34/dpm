//! Reviewed plan changes recorded as entity deltas rather than whole proposed plans.
//!
//! The operation log stores only what a review changed: each [`EntityChange`] names one entity
//! with its full `before` and `after` values. Replaying the change checks every `before` against
//! the state it is applied to, so a delta never silently lands on a state it was not computed for.
//!
//! Artifacts are not part of the delta. Evidence is attributed to its author and changes only
//! through the evidence command, so a proposal that alters artifacts is refused before a delta is
//! built, and the delta leaves the recorded artifacts exactly as they were.
//!
//! The difference identifies edges by ID and compares links as a set, so it cannot carry their
//! order. A patch keeps retained edges and links in their current order, edits edges in place and
//! appends additions in the order of the changes. Order leaves every schedule number unchanged but
//! is the display order of lists built from these collections, such as critical activities, the
//! dependencies `explain` lists and the exported JSON.

use super::{EntityChange, invalid};
use crate::{Command, EngineError, Operation};
use chrono::{DateTime, Utc};
use dpm_model::{ActorId, ActorKind, OperationId, Plan};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// Refuse what no reviewed plan change may do, before any difference is computed.
pub(crate) fn authorize(actor: &ActorId, reason: &str) -> Result<(), EngineError> {
    if reason.trim().is_empty() {
        return Err(invalid("plan change", "reason must not be empty"));
    }
    if actor.kind == ActorKind::Agent {
        return Err(EngineError::ActorNotAllowed {
            actor: actor.clone(),
            action: "approve plan scope",
        });
    }
    Ok(())
}

/// Turn a reviewed full proposal into the delta command the operation log records.
///
/// The proposal is validated exactly as [`crate::propose_change`] validates it, which also refuses
/// what a delta cannot carry: a stale revision, another format, or changed artifacts.
pub fn plan_change(
    current: &Plan,
    proposed: &Plan,
    reason: impl Into<String>,
) -> Result<Command, EngineError> {
    let preview = crate::propose_change(current, proposed)?;
    Ok(Command::ApplyChange {
        changes: preview.changes,
        reason: reason.into(),
    })
}

/// Apply a reviewed full proposal as one delta operation.
///
/// The actor and reason are checked before the proposal is compared, as the command itself checks
/// them first, so every refusal is the one applying the full proposal would produce.
pub fn apply_plan_change(
    plan: &mut Plan,
    actor: ActorId,
    proposed: &Plan,
    reason: impl Into<String>,
    timestamp: DateTime<Utc>,
    id: OperationId,
) -> Result<Operation, EngineError> {
    let reason = reason.into();
    authorize(&actor, &reason)?;
    let command = plan_change(plan, proposed, reason)?;
    crate::apply_command(plan, actor, command, timestamp, id)
}

/// Apply entity changes to `current` without any policy check.
///
/// Every change's `before` must equal the entity in `current` (null for an addition), otherwise
/// the delta was computed against another state and [`EngineError::StaleChange`] is returned.
/// Each entity may appear once. Validation and protection are the caller's responsibility.
pub fn patch(current: &Plan, changes: &[EntityChange]) -> Result<Plan, EngineError> {
    let mut value = serde_json::to_value(current)?;
    let mut seen = BTreeSet::new();
    for change in changes {
        if !seen.insert((change.collection.as_str(), change.id.as_deref())) {
            return Err(invalid(
                describe(change),
                "a plan change names each entity at most once",
            ));
        }
        apply_one(&mut value, current, change)?;
    }
    Ok(serde_json::from_value(value)?)
}

fn apply_one(value: &mut Value, current: &Plan, change: &EntityChange) -> Result<(), EngineError> {
    let root = value
        .as_object_mut()
        .ok_or_else(|| invalid("plan", "a plan serializes as an object"))?;
    match (change.collection.as_str(), change.id.as_deref()) {
        ("workspace", None) => {
            let slot = root.entry("workspace").or_insert(Value::Null);
            replace(slot, change)
        }
        ("links", None) => patch_links(root, current, change),
        ("calendars", None) => {
            let found = root.get("calendars").cloned().unwrap_or(Value::Null);
            fresh(&found, change)?;
            if change.after.is_null() {
                root.remove("calendars");
            } else {
                root.insert("calendars".into(), change.after.clone());
            }
            Ok(())
        }
        ("dependencies", Some(id)) => patch_edges(root, id, change),
        (collection, Some(id)) if super::KEYED_COLLECTIONS.contains(&collection) => {
            let map = root
                .entry(collection)
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .ok_or_else(|| invalid(collection, "a keyed collection serializes as an object"))?;
            let found = map.get(id).cloned().unwrap_or(Value::Null);
            fresh(&found, change)?;
            if change.after.is_null() {
                map.remove(id);
            } else {
                map.insert(id.to_owned(), change.after.clone());
            }
            Ok(())
        }
        _ => Err(invalid(describe(change), "unknown plan change target")),
    }
}

fn replace(slot: &mut Value, change: &EntityChange) -> Result<(), EngineError> {
    fresh(slot, change)?;
    if change.after.is_null() {
        return Err(invalid(describe(change), "a workspace cannot be removed"));
    }
    *slot = change.after.clone();
    Ok(())
}

fn patch_edges(
    root: &mut Map<String, Value>,
    id: &str,
    change: &EntityChange,
) -> Result<(), EngineError> {
    let edges = root
        .entry("dependencies")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| invalid("dependencies", "dependencies serialize as an array"))?;
    let position = edges
        .iter()
        .position(|edge| edge.get("id").and_then(Value::as_str) == Some(id));
    let found = position
        .and_then(|index| edges.get(index))
        .cloned()
        .unwrap_or(Value::Null);
    fresh(&found, change)?;
    match (position, change.after.is_null()) {
        (Some(index), true) => {
            edges.remove(index);
        }
        (Some(index), false) => {
            if let Some(slot) = edges.get_mut(index) {
                *slot = change.after.clone();
            }
        }
        (None, false) => edges.push(change.after.clone()),
        (None, true) => {}
    }
    Ok(())
}

/// Links are compared as the sorted set the difference records; retained links keep their order.
fn patch_links(
    root: &mut Map<String, Value>,
    current: &Plan,
    change: &EntityChange,
) -> Result<(), EngineError> {
    fresh(&super::sorted_links(current)?, change)?;
    let wanted = change.after.as_array().cloned().unwrap_or_default();
    let existing = root
        .get("links")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut links: Vec<Value> = existing
        .into_iter()
        .filter(|link| wanted.contains(link))
        .collect();
    let added: Vec<Value> = wanted
        .into_iter()
        .filter(|link| !links.contains(link))
        .collect();
    links.extend(added);
    root.insert("links".into(), Value::Array(links));
    Ok(())
}

fn fresh(found: &Value, change: &EntityChange) -> Result<(), EngineError> {
    if *found == change.before {
        return Ok(());
    }
    Err(EngineError::StaleChange {
        collection: change.collection.clone(),
        id: change.id.clone(),
    })
}

fn describe(change: &EntityChange) -> String {
    match &change.id {
        Some(id) => format!("{} {id}", change.collection),
        None => change.collection.clone(),
    }
}

#[cfg(test)]
mod tests;
