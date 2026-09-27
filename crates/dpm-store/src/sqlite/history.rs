use super::{
    SqliteStore, load_blocking,
    snapshot::{column, decode, revision_from_sql},
};
use crate::{StoreError, StoredRecord, error::database_error};
use dpm_engine::Operation;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One immutable operation addressed by its local append order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Local database cursor, independent of wrapping domain revisions.
    pub sequence: u64,
    /// Actor, timestamp, revision precondition and semantic command.
    pub operation: Operation,
}

/// A bounded operation page read with a consistent authoritative revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryPage {
    /// Revision visible in the same SQLite transaction.
    pub revision: u64,
    /// Operations in ascending append order.
    pub entries: Vec<HistoryEntry>,
    /// Last returned sequence, suitable for the next request; unchanged for an empty page.
    pub next_after_sequence: u64,
}

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
        let plan = load_blocking(&transaction, &self.path)?
            .ok_or_else(|| StoreError::NotInitialized(self.path.clone()))?;
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
            next_after_sequence: entries
                .last()
                .map_or(after_sequence, |entry| entry.sequence),
            entries,
        })
    }
}

/// Selects the columns [`read_entry`] expects, in its order.
pub(crate) const OPERATION_COLUMNS: &str = "SELECT sequence, operation_id, base_revision, \
     resulting_revision, actor_json, timestamp, command_json FROM operations";

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
        operation: Operation {
            id: decode(&scalar(1, "operation_id")?, path, record, "operation_id")?,
            base_revision: revision(2, "base_revision")?,
            resulting_revision: revision(3, "resulting_revision")?,
            actor: decode(&text(4, "actor_json")?, path, record, "actor_json")?,
            timestamp: decode(&scalar(5, "timestamp")?, path, record, "timestamp")?,
            command: decode(&text(6, "command_json")?, path, record, "command_json")?,
        },
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
