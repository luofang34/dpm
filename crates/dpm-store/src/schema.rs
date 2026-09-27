//! Database layout version stored in SQLite's `user_version` header field.
//!
//! Policy: every accepted header value names one exact layout, compared object by object against
//! `sqlite_master`, so a planted trigger, view or index is refused before any statement that could
//! fire it. Version 2 is the `plan_state` + `operations` + `history_origin` layout. Headers 0 (the
//! unversioned baseline) and 1 name the same layout without `history_origin`; such a store is read
//! as is and upgraded by its next write transaction, which records the origin it derives from the
//! history, so read-only commands never write. A newer version is refused before any write. A
//! future layout change raises the version, adds an upgrade that runs inside the first write
//! transaction, and must also change the layout in a way that binaries predating the version
//! header fail on, because those binaries never read the header.

use crate::{StoreError, error::database_error};
use rusqlite::Connection;
use std::path::Path;

mod layout;

/// Database layout version this binary reads and writes.
pub const SCHEMA_VERSION: i64 = 2;

const BASELINE_VERSION: i64 = 0;
const ORIGINLESS_VERSION: i64 = 1;

/// Table layout found in a database whose version this binary accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// No schema objects yet: a new file awaiting initialization.
    Empty,
    /// Snapshot and operation tables written before history origins were recorded.
    Originless,
    /// The current layout, including the recorded history origin.
    Current,
}

pub(crate) fn stored_version_blocking(
    connection: &Connection,
    path: &Path,
) -> Result<i64, StoreError> {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(database_error(path, "read schema version"))
}

/// Reject unknown versions and any layout other than the exact one the version names, using reads
/// only.
pub(crate) fn check_blocking(connection: &Connection, path: &Path) -> Result<Layout, StoreError> {
    let found = stored_version_blocking(connection, path)?;
    let (expected, unstamped) = match found {
        BASELINE_VERSION => (Layout::Originless, true),
        ORIGINLESS_VERSION => (Layout::Originless, false),
        SCHEMA_VERSION => (Layout::Current, false),
        _ => {
            return Err(StoreError::UnsupportedSchemaVersion {
                path: path.to_path_buf(),
                found,
                supported: SCHEMA_VERSION,
            });
        }
    };
    let objects = layout::objects_blocking(connection, path)?;
    // Only an unstamped file may be empty; a stamped header without tables was not written by DPM.
    if objects.is_empty() && unstamped {
        return Ok(Layout::Empty);
    }
    layout::compare(path, found, &objects, layout::expected(expected))?;
    Ok(expected)
}

/// Create the current tables in an empty database and stamp its version, inside a caller-owned
/// write transaction.
pub(crate) fn create_blocking(connection: &Connection, path: &Path) -> Result<(), StoreError> {
    connection
        .execute_batch(&format!(
            "{}; {}; {}; PRAGMA user_version = {SCHEMA_VERSION};",
            layout::PLAN_STATE,
            layout::OPERATIONS,
            layout::HISTORY_ORIGIN
        ))
        .map_err(database_error(path, "initialize schema"))
}

/// Add the history origin table to an originless store and stamp the current version, inside a
/// caller-owned write transaction so the upgrade commits or rolls back with that write.
pub(crate) fn upgrade_blocking(connection: &Connection, path: &Path) -> Result<(), StoreError> {
    connection
        .execute_batch(&format!(
            "{}; PRAGMA user_version = {SCHEMA_VERSION};",
            layout::HISTORY_ORIGIN
        ))
        .map_err(database_error(path, "upgrade schema"))
}

#[cfg(test)]
pub(crate) use layout::{OPERATIONS as TEST_OPERATIONS_DDL, PLAN_STATE as TEST_PLAN_STATE_DDL};

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
