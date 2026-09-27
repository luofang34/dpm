//! Consistency checks shared by verification and by the upgrade that records a history origin.

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
    /// Base revision of the first operation, absent without operations.
    pub(super) first_base_revision: Option<u64>,
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
/// from `origin` (when recorded) to `snapshot` one operation at a time.
pub(super) fn check_history_blocking(
    connection: &Connection,
    path: &Path,
    snapshot: u64,
    origin: Option<u64>,
) -> Result<HistorySummary, StoreError> {
    let mut statement = connection
        .prepare(&format!("{OPERATION_COLUMNS} ORDER BY sequence"))
        .map_err(database_error(path, "prepare history verification"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "query history"))?;
    let mut summary = HistorySummary {
        count: 0,
        first_base_revision: None,
    };
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
        if let Some(expected) = previous_revision
            && expected != operation.base_revision
        {
            return Err(breaks(expected, operation.base_revision));
        }
        let expected = operation.base_revision.wrapping_add(1);
        if operation.resulting_revision != expected {
            return Err(breaks(expected, operation.resulting_revision));
        }
        summary
            .first_base_revision
            .get_or_insert(operation.base_revision);
        previous_sequence = Some(entry.sequence);
        previous_revision = Some(operation.resulting_revision);
        summary.count = summary.count.wrapping_add(1);
    }
    match (previous_revision, summary.count) {
        (Some(origin), 0) if origin != snapshot => Err(StoreError::MissingHistory {
            path: path.to_path_buf(),
            origin,
            snapshot,
        }),
        (Some(last_operation), _) if last_operation != snapshot => {
            Err(StoreError::SnapshotRevisionMismatch {
                path: path.to_path_buf(),
                snapshot,
                last_operation,
            })
        }
        _ => Ok(summary),
    }
}
