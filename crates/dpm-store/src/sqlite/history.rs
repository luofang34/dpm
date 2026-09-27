use super::{SqliteStore, load_blocking};
use crate::{StoreError, error::database_error};
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

/// Decode one row selected with [`OPERATION_COLUMNS`].
pub(crate) fn read_entry(row: &rusqlite::Row<'_>, path: &Path) -> Result<HistoryEntry, StoreError> {
    let get_string = |index| {
        row.get::<_, String>(index)
            .map_err(database_error(path, "read operation field"))
    };
    let get_revision = |index| {
        row.get::<_, i64>(index)
            .map(|n| u64::from_ne_bytes(n.to_ne_bytes()))
            .map_err(database_error(path, "read operation revision"))
    };
    Ok(HistoryEntry {
        sequence: row
            .get(0)
            .map_err(database_error(path, "read history cursor"))?,
        operation: Operation {
            id: serde_json::from_value(serde_json::Value::String(get_string(1)?))?,
            base_revision: get_revision(2)?,
            resulting_revision: get_revision(3)?,
            actor: serde_json::from_str(&get_string(4)?)?,
            timestamp: serde_json::from_value(serde_json::Value::String(get_string(5)?))?,
            command: serde_json::from_str(&get_string(6)?)?,
        },
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
