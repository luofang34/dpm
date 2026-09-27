use crate::{StoreError, StoredRecord, error::database_error};
use dpm_engine::Operation;
use dpm_model::Plan;
use rusqlite::{Connection, params};
use std::path::Path;

pub(super) fn revision_to_sql(value: u64) -> i64 {
    // Bit-preserving storage lets SQLite INTEGER round-trip the full wrapping u64 domain.
    i64::from_ne_bytes(value.to_ne_bytes())
}

pub(super) fn revision_from_sql(value: i64) -> u64 {
    u64::from_ne_bytes(value.to_ne_bytes())
}

/// Decode one column of a stored record, attributing a type mismatch to that record.
pub(super) fn column<T: rusqlite::types::FromSql>(
    row: &rusqlite::Row<'_>,
    index: usize,
    path: &Path,
    record: StoredRecord,
    field: &'static str,
) -> Result<T, StoreError> {
    row.get(index).map_err(|source| StoreError::CorruptColumn {
        path: path.to_path_buf(),
        record,
        field,
        source,
    })
}

/// Decode stored JSON, attributing a failure to its record rather than to the caller's request.
pub(super) fn decode<T: serde::de::DeserializeOwned>(
    json: &str,
    path: &Path,
    record: StoredRecord,
    field: &'static str,
) -> Result<T, StoreError> {
    serde_json::from_str(json).map_err(|source| StoreError::CorruptJson {
        path: path.to_path_buf(),
        record,
        field,
        source,
    })
}

pub(super) fn load_blocking(
    connection: &Connection,
    path: &Path,
) -> Result<Option<Plan>, StoreError> {
    let mut statement = connection
        .prepare("SELECT revision, plan_json FROM plan_state WHERE singleton = 1")
        .map_err(database_error(path, "prepare snapshot query"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "load snapshot"))?;
    let Some(row) = rows.next().map_err(database_error(path, "load snapshot"))? else {
        return Ok(None);
    };
    let record = StoredRecord::Snapshot;
    let stored_revision = revision_from_sql(column(row, 0, path, record, "revision")?);
    let json: String = column(row, 1, path, record, "plan_json")?;
    let plan: Plan = decode(&json, path, record, "plan_json")?;
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

/// The revision the stored history starts from, as recorded when the store was initialized or
/// upgraded; `None` when no origin has been recorded.
pub(super) fn origin_blocking(
    connection: &Connection,
    path: &Path,
) -> Result<Option<u64>, StoreError> {
    let mut statement = connection
        .prepare("SELECT revision FROM history_origin WHERE singleton = 1")
        .map_err(database_error(path, "prepare history origin query"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "load history origin"))?;
    match rows
        .next()
        .map_err(database_error(path, "load history origin"))?
    {
        Some(row) => Ok(Some(revision_from_sql(column(
            row,
            0,
            path,
            StoredRecord::Origin,
            "revision",
        )?))),
        None => Ok(None),
    }
}

pub(super) fn write_origin_blocking(
    connection: &Connection,
    path: &Path,
    origin: u64,
) -> Result<(), StoreError> {
    connection
        .execute(
            "INSERT INTO history_origin(singleton, revision) VALUES(1, ?1)",
            [revision_to_sql(origin)],
        )
        .map_err(database_error(path, "record history origin"))?;
    Ok(())
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
