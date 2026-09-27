//! Page and history-continuity checks run by verification before the history is replayed.

use super::history::{OPERATION_COLUMNS, read_entry};
use crate::{StoreError, error::database_error};
use rusqlite::Connection;
use std::path::Path;

const PROBLEM_LIMIT: usize = 10;

/// What a consistent operation history consists of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HistorySummary {
    /// Number of committed operations.
    pub(super) count: u64,
}

pub(super) fn integrity_check_blocking(
    connection: &Connection,
    path: &Path,
) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare("PRAGMA integrity_check")
        .map_err(database_error(path, "prepare integrity check"))?;
    let problems = statement
        .query_map([], |row| row.get(0))
        .map_err(database_error(path, "run integrity check"))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(database_error(path, "read integrity check"))?;
    if problems == ["ok"] {
        return Ok(());
    }
    Err(StoreError::IntegrityCheckFailed {
        path: path.to_path_buf(),
        problems: problems.into_iter().take(PROBLEM_LIMIT).collect(),
    })
}

/// Check that every operation decodes, that local sequences have no gaps, and that revisions run
/// from the genesis revision `origin` to `snapshot` one operation at a time.
pub(super) fn check_history_blocking(
    connection: &Connection,
    path: &Path,
    snapshot: u64,
    origin: u64,
) -> Result<HistorySummary, StoreError> {
    let mut statement = connection
        .prepare(&format!("{OPERATION_COLUMNS} ORDER BY sequence"))
        .map_err(database_error(path, "prepare history verification"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "query history"))?;
    let mut summary = HistorySummary { count: 0 };
    let (mut previous_sequence, mut previous_revision) = (None::<u64>, origin);
    while let Some(row) = rows.next().map_err(database_error(path, "read history"))? {
        let entry = read_entry(row, path)?;
        if let Some(previous) = previous_sequence
            && entry.sequence != previous.wrapping_add(1)
        {
            return Err(StoreError::SequenceGap {
                path: path.to_path_buf(),
                previous,
                sequence: entry.sequence,
            });
        }
        let operation = &entry.operation;
        let breaks = |expected, found| StoreError::HistoryDiscontinuity {
            path: path.to_path_buf(),
            sequence: entry.sequence,
            expected,
            found,
        };
        if previous_revision != operation.base_revision {
            return Err(breaks(previous_revision, operation.base_revision));
        }
        let expected = operation.base_revision.wrapping_add(1);
        if operation.resulting_revision != expected {
            return Err(breaks(expected, operation.resulting_revision));
        }
        previous_sequence = Some(entry.sequence);
        previous_revision = operation.resulting_revision;
        summary.count = summary.count.wrapping_add(1);
    }
    match summary.count {
        _ if previous_revision == snapshot => Ok(summary),
        0 => Err(StoreError::MissingHistory {
            path: path.to_path_buf(),
            origin,
            snapshot,
        }),
        _ => Err(StoreError::SnapshotRevisionMismatch {
            path: path.to_path_buf(),
            snapshot,
            last_operation: previous_revision,
        }),
    }
}
