use super::{
    SqliteStore, lineage,
    snapshot::{column, decode, load_cached_blocking, revision_from_sql},
};
use crate::{
    HistoryEntry, HistoryPage, RecordedOperation, StoreError, StoredRecord, error::database_error,
};
use dpm_engine::Operation;
use dpm_model::OperationId;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

impl SqliteStore {
    /// Read at most 1000 operations after a local sequence cursor without modifying history.
    pub fn history_blocking(
        &self,
        after_sequence: u64,
        limit: u16,
    ) -> Result<HistoryPage, StoreError> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(database_error(&self.path, "begin history query"))?;
        // The page's revision comes from the snapshot in this transaction; the decode cache makes
        // that a byte comparison while the snapshot is unchanged.
        let plan = load_cached_blocking(&transaction, &self.path, &self.cache)?
            .ok_or_else(|| StoreError::NotInitialized(self.path.clone()))?;
        let lineage = lineage::read_blocking(&transaction, &self.path)?;
        let mut entries = Vec::new();
        if let Ok(cursor) = i64::try_from(after_sequence) {
            let mut statement = transaction
                .prepare(&format!(
                    "{OPERATION_COLUMNS} WHERE sequence > ?1 ORDER BY sequence LIMIT ?2"
                ))
                .map_err(database_error(&self.path, "prepare history query"))?;
            let mut rows = statement
                .query(params![cursor, limit.min(1000)])
                .map_err(database_error(&self.path, "query history"))?;
            while let Some(row) = rows
                .next()
                .map_err(database_error(&self.path, "read history"))?
            {
                entries.push(read_entry(row, &self.path)?);
            }
        }
        transaction
            .commit()
            .map_err(database_error(&self.path, "finish history query"))?;
        Ok(HistoryPage {
            revision: plan.revision,
            lineage_id: Some(lineage.lineage_id),
            next_after_sequence: entries
                .last()
                .map_or(after_sequence, |entry| entry.sequence),
            entries,
        })
    }

    /// The operation recorded under this identity, if any, whichever lineage recorded it.
    pub fn recorded_operation_blocking(
        &self,
        id: OperationId,
    ) -> Result<Option<HistoryEntry>, StoreError> {
        recorded_blocking(&self.connection, &self.path, id)
    }
}

/// Look an operation identity up through the unique index on `operation_id`.
pub(super) fn recorded_blocking(
    connection: &Connection,
    path: &Path,
    id: OperationId,
) -> Result<Option<HistoryEntry>, StoreError> {
    let mut statement = connection
        .prepare(&format!("{OPERATION_COLUMNS} WHERE operation_id = ?1"))
        .map_err(database_error(path, "prepare operation lookup"))?;
    statement
        .query_row(params![id.to_string()], |row| Ok(read_entry(row, path)))
        .optional()
        .map_err(database_error(path, "look up operation"))?
        .transpose()
}

/// Selects the columns [`read_entry`] expects, in its order.
pub(crate) const OPERATION_COLUMNS: &str = "SELECT sequence, operation_id, base_revision, \
     resulting_revision, actor_json, timestamp, command_json, workspace_id, lineage_id \
     FROM operations";

/// Decode one row selected with [`OPERATION_COLUMNS`], attributing damage to its sequence.
pub(crate) fn read_entry(row: &rusqlite::Row<'_>, path: &Path) -> Result<HistoryEntry, StoreError> {
    let sequence: u64 = row
        .get(0)
        .map_err(database_error(path, "read history cursor"))?;
    let record = StoredRecord::Operation { sequence };
    let text = |index, field| column::<String>(row, index, path, record, field);
    let revision =
        |index, field| column::<i64>(row, index, path, record, field).map(revision_from_sql);
    // Identifier and timestamp columns hold bare strings; decoding them as JSON strings reuses
    // the serde formats the operation itself declares.
    let scalar =
        |index, field| text(index, field).map(|value| serde_json::Value::String(value).to_string());
    Ok(HistoryEntry {
        sequence,
        operation: RecordedOperation {
            operation: Operation {
                id: decode(&scalar(1, "operation_id")?, path, record, "operation_id")?,
                base_revision: revision(2, "base_revision")?,
                resulting_revision: revision(3, "resulting_revision")?,
                actor: decode(&text(4, "actor_json")?, path, record, "actor_json")?,
                timestamp: decode(&scalar(5, "timestamp")?, path, record, "timestamp")?,
                command: decode(&text(6, "command_json")?, path, record, "command_json")?,
            },
            workspace_id: decode(&scalar(7, "workspace_id")?, path, record, "workspace_id")?,
            lineage_id: decode(&scalar(8, "lineage_id")?, path, record, "lineage_id")?,
        },
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
