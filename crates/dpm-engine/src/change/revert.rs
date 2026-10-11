use super::{ChangePreview, EntityChange};
use crate::{Command, EngineError};
use dpm_model::{OperationId, Plan};

/// Review the change that undoes a recorded plan change, against the current plan.
///
/// The inverse of a recorded change swaps each entity's `before` and `after`, in reverse order. It applies only
/// where every entity it names is still as the recorded change left it; otherwise each later-changed entity is
/// named. The inverse is then validated like any proposal, so protected execution and the graph's invariants
/// refuse it the same way, and the preview's `changes` are what [`Command::RevertChange`] must carry.
pub fn propose_revert(
    current: &Plan,
    operation: OperationId,
    recorded: &Command,
) -> Result<ChangePreview, EngineError> {
    let changes = match recorded {
        Command::ApplyChange { changes, .. } | Command::RevertChange { changes, .. } => changes,
        _ => {
            return Err(EngineError::InvalidCommand {
                entity: format!("operation {operation}"),
                reason: "only a recorded plan change can be reverted".into(),
            });
        }
    };
    let state = serde_json::to_value(current)?;
    let inverse: Vec<EntityChange> = changes
        .iter()
        .rev()
        .map(|change| EntityChange {
            before: as_applied(&state, change),
            after: change.before.clone(),
            ..change.clone()
        })
        .collect();
    let mut conflicts = Vec::new();
    for change in &inverse {
        match super::patch(current, std::slice::from_ref(change)) {
            Err(EngineError::StaleChange { .. }) => conflicts.push(name(current, change)),
            Err(other) => return Err(other),
            Ok(_) => {}
        }
    }
    if !conflicts.is_empty() {
        return Err(EngineError::InvalidCommand {
            entity: format!("revert of operation {operation}"),
            reason: format!(
                "{} changed after it, by a later plan change or a lifecycle event; revert later plan changes first, or propose a new change",
                conflicts.join(", ")
            ),
        });
    }
    let candidate = super::patch(current, &inverse)?;
    super::propose_change(current, &candidate)
}

/// The state a recorded change left an entity in. Applying a change stamps when each decision it adds was
/// resolved, which its recorded `after` cannot carry, so that one field is read from the current plan.
fn as_applied(state: &serde_json::Value, change: &EntityChange) -> serde_json::Value {
    let mut after = change.after.clone();
    if change.collection == "decisions" && change.before.is_null() {
        let stamped = change
            .id
            .as_deref()
            .and_then(|id| state.get("decisions")?.get(id)?.get("resolved_at"))
            .cloned();
        if let (Some(stamped), Some(fields)) = (stamped, after.as_object_mut()) {
            fields.insert("resolved_at".into(), stamped);
        }
    }
    after
}

/// The entity a change names, with its key where it has one.
fn name(current: &Plan, change: &EntityChange) -> String {
    let id = change.id.as_deref().unwrap_or("");
    let key = (change.collection == "work_items")
        .then(|| current.work_items.values().find(|w| w.id.to_string() == id))
        .flatten()
        .map(|w| format!(" ({})", w.key));
    match &change.id {
        Some(id) => format!("{} {id}{}", change.collection, key.unwrap_or_default()),
        None => change.collection.clone(),
    }
}

#[cfg(test)]
mod tests;
