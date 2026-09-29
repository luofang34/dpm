//! Rebuild the snapshot from the genesis plan through the engine, as verification's final check.
//!
//! Each operation is re-executed with its recorded identity, actor and time, so an honest history
//! reproduces the snapshot exactly. A difference is reported, never repaired: which of the two is
//! right can only be decided by an operator holding a backup.

use super::history::{OPERATION_COLUMNS, read_entry};
use crate::{StoreError, error::database_error};
use dpm_engine::apply_command;
use dpm_model::Plan;
use rusqlite::Connection;
use std::path::Path;

/// Replay every operation in append order onto `genesis` and compare the result with `snapshot`.
pub(super) fn replay_blocking(
    connection: &Connection,
    path: &Path,
    genesis: Plan,
    snapshot: &Plan,
) -> Result<(), StoreError> {
    let plan = replay_before_blocking(connection, path, genesis, u64::MAX)?;
    if plan == *snapshot {
        return Ok(());
    }
    Err(StoreError::ReplayDiverged {
        path: path.to_path_buf(),
        revision: snapshot.revision,
        differing: differing_fields(&plan, snapshot)?,
    })
}

/// The plan as it stood before the operation at local sequence `before`, rebuilt from `genesis`.
pub(super) fn replay_before_blocking(
    connection: &Connection,
    path: &Path,
    genesis: Plan,
    before: u64,
) -> Result<Plan, StoreError> {
    let bound = i64::try_from(before).unwrap_or(i64::MAX);
    let mut statement = connection
        .prepare(&format!(
            "{OPERATION_COLUMNS} WHERE sequence < ?1 ORDER BY sequence"
        ))
        .map_err(database_error(path, "prepare history replay"))?;
    let mut rows = statement
        .query([bound])
        .map_err(database_error(path, "query history"))?;
    let mut plan = genesis;
    while let Some(row) = rows.next().map_err(database_error(path, "read history"))? {
        let entry = read_entry(row, path)?;
        let operation = entry.operation.operation;
        apply_command(
            &mut plan,
            operation.actor,
            operation.command,
            operation.timestamp,
            operation.id,
        )
        .map_err(|source| StoreError::ReplayRefused {
            path: path.to_path_buf(),
            sequence: entry.sequence,
            source,
        })?;
    }
    Ok(plan)
}

/// Top-level plan fields whose serialized values differ, so an operator knows where to look.
fn differing_fields(replayed: &Plan, snapshot: &Plan) -> Result<Vec<String>, StoreError> {
    let replayed = serde_json::to_value(replayed)?;
    let snapshot = serde_json::to_value(snapshot)?;
    let empty = serde_json::Map::new();
    let (left, right) = (
        replayed.as_object().unwrap_or(&empty),
        snapshot.as_object().unwrap_or(&empty),
    );
    let mut fields: Vec<String> = left
        .keys()
        .chain(right.keys())
        .filter(|key| left.get(*key) != right.get(*key))
        .cloned()
        .collect();
    fields.sort();
    fields.dedup();
    Ok(fields)
}
