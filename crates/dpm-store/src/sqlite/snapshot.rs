use crate::{StoreError, error::database_error};
use dpm_engine::Operation;
use dpm_model::Plan;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

pub(super) fn revision_to_sql(value: u64) -> i64 {
    // Bit-preserving storage lets SQLite INTEGER round-trip the full wrapping u64 domain.
    i64::from_ne_bytes(value.to_ne_bytes())
}

pub(super) fn load_blocking(
    connection: &Connection,
    path: &Path,
) -> Result<Option<Plan>, StoreError> {
    let row: Option<(i64, String)> = connection
        .query_row(
            "SELECT revision, plan_json FROM plan_state WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database_error(path, "load snapshot"))?;
    let Some((revision, json)) = row else {
        return Ok(None);
    };
    let plan: Plan = serde_json::from_str(&json)?;
    let stored_revision = u64::from_ne_bytes(revision.to_ne_bytes());
    if stored_revision != plan.revision {
        return Err(StoreError::CorruptSnapshot {
            path: path.to_path_buf(),
            row: stored_revision,
            json: plan.revision,
        });
    }
    plan.validate()?;
    Ok(Some(plan))
}

pub(super) fn write_operation_blocking(
    connection: &Connection,
    path: &Path,
    plan: &Plan,
    operation: &Operation,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO operations(operation_id, base_revision, resulting_revision, actor_json, timestamp, command_json)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        params![operation.id.to_string(), revision_to_sql(operation.base_revision),
            revision_to_sql(operation.resulting_revision), serde_json::to_string(&operation.actor)?,
            operation.timestamp.to_rfc3339(), serde_json::to_string(&operation.command)?],
    ).map_err(database_error(path, "append operation"))?;
    connection
        .execute(
            "UPDATE plan_state SET revision = ?1, plan_json = ?2 WHERE singleton = 1",
            params![revision_to_sql(plan.revision), serde_json::to_string(plan)?],
        )
        .map_err(database_error(path, "update snapshot"))?;
    Ok(())
}
