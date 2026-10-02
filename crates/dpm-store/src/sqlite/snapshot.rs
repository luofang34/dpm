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

/// The snapshot's revision column alone; the JSON beside it is neither read nor decoded.
pub(super) fn revision_blocking(
    connection: &Connection,
    path: &Path,
) -> Result<Option<u64>, StoreError> {
    let mut statement = connection
        .prepare("SELECT revision FROM plan_state WHERE singleton = 1")
        .map_err(database_error(path, "prepare revision query"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "read revision"))?;
    let Some(row) = rows.next().map_err(database_error(path, "read revision"))? else {
        return Ok(None);
    };
    let revision = column(row, 0, path, StoredRecord::Snapshot, "revision")?;
    Ok(Some(revision_from_sql(revision)))
}

pub(super) fn load_blocking(
    connection: &Connection,
    path: &Path,
) -> Result<Option<Plan>, StoreError> {
    let query = "SELECT revision, snapshot_json FROM plan_state WHERE singleton = 1";
    load_plan_blocking(connection, path, query, StoredRecord::Snapshot, None)
}

/// The plan the operation history replays from; `None` for an uninitialized store.
pub(super) fn genesis_blocking(
    connection: &Connection,
    path: &Path,
) -> Result<Option<Plan>, StoreError> {
    let query = "SELECT revision, plan_json FROM genesis WHERE singleton = 1";
    load_plan_blocking(connection, path, query, StoredRecord::Genesis, None)
}

pub(super) fn load_cached_blocking(
    connection: &Connection,
    path: &Path,
    cache: &super::snapshot_cache::SnapshotCache,
) -> Result<Option<Plan>, StoreError> {
    let query = "SELECT revision, snapshot_json FROM plan_state WHERE singleton = 1";
    load_plan_blocking(connection, path, query, StoredRecord::Snapshot, Some(cache))
}

fn load_plan_blocking(
    connection: &Connection,
    path: &Path,
    query: &str,
    record: StoredRecord,
    cache: Option<&super::snapshot_cache::SnapshotCache>,
) -> Result<Option<Plan>, StoreError> {
    let mut statement = connection
        .prepare(query)
        .map_err(database_error(path, "prepare plan query"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "load plan"))?;
    let Some(row) = rows.next().map_err(database_error(path, "load plan"))? else {
        return Ok(None);
    };
    let stored_revision = revision_from_sql(column(row, 0, path, record, "revision")?);
    let field = match record {
        StoredRecord::Snapshot => "snapshot_json",
        StoredRecord::Genesis
        | StoredRecord::Operation { .. }
        | StoredRecord::Lineage
        | StoredRecord::Run => "plan_json",
    };
    let json: String = column(row, 1, path, record, field)?;
    let plan: Plan = match cache {
        Some(cache) => cache.decode(&json, path)?,
        None => {
            let plan: Plan = decode(&json, path, record, field)?;
            plan.validate()?;
            plan
        }
    };
    if stored_revision != plan.revision {
        return Err(StoreError::CorruptSnapshot {
            path: path.to_path_buf(),
            record,
            row: stored_revision,
            json: plan.revision,
        });
    }
    Ok(Some(plan))
}

/// Record the initial snapshot, the genesis plan the history replays from and a new lineage.
pub(super) fn write_initial_blocking(
    connection: &Connection,
    path: &Path,
    plan: &Plan,
) -> Result<(), StoreError> {
    let json = serde_json::to_string(plan)?;
    let revision = revision_to_sql(plan.revision);
    connection
        .execute(
            "INSERT INTO plan_state(singleton, revision, snapshot_json) VALUES(1, ?1, ?2)",
            params![revision, json],
        )
        .map_err(database_error(path, "initialize snapshot"))?;
    connection
        .execute(
            "INSERT INTO genesis(singleton, revision, plan_json) VALUES(1, ?1, ?2)",
            params![revision, json],
        )
        .map_err(database_error(path, "record genesis plan"))?;
    super::lineage::write_initial_blocking(connection, path)
}

/// Append `operation` under the store's current lineage and return its local sequence.
pub(super) fn write_operation_blocking(
    connection: &Connection,
    path: &Path,
    plan: &Plan,
    operation: &Operation,
    lineage: dpm_model::LineageId,
) -> Result<u64, StoreError> {
    connection
        .execute(
            "INSERT INTO operations(operation_id, workspace_id, lineage_id, base_revision,
             resulting_revision, actor_json, timestamp, command_json)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                operation.id.to_string(),
                plan.workspace.id.to_string(),
                lineage.to_string(),
                revision_to_sql(operation.base_revision),
                revision_to_sql(operation.resulting_revision),
                serde_json::to_string(&operation.actor)?,
                operation.timestamp.to_rfc3339(),
                serde_json::to_string(&operation.command)?
            ],
        )
        .map_err(database_error(path, "append operation"))?;
    let sequence = revision_from_sql(connection.last_insert_rowid());
    connection
        .execute(
            "UPDATE plan_state SET revision = ?1, snapshot_json = ?2 WHERE singleton = 1",
            params![revision_to_sql(plan.revision), serde_json::to_string(plan)?],
        )
        .map_err(database_error(path, "update snapshot"))?;
    Ok(sequence)
}
