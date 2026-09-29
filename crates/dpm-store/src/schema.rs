//! Database layout version stored in SQLite's `user_version` header field.
//!
//! Policy: every accepted header value names one exact layout, compared object by object against
//! `sqlite_master`, so a planted trigger, view or index is refused before any statement that could
//! fire it. Version 4 is the only layout this binary reads or writes: the `plan_state` snapshot,
//! the `operations` log of entity-level plan deltas carrying their workspace and lineage, the
//! `genesis` plan every operation replays from, and the `store_lineage` of the file. Versions 1
//! and 2, and a header of 0 over DPM tables, name retired layouts whose logs record whole proposed
//! plans and cannot be replayed; version 3 records no lineage, so a copy of it could not be told
//! apart from its source. They are refused with guidance and never modified. A newer version is refused before any write. A future layout change raises the
//! version and must also change the layout in a way that binaries predating the version header
//! fail on, because those binaries never read the header: they read `plan_state.plan_json`, which
//! this layout names `snapshot_json`.

use crate::{StoreError, error::database_error};
use rusqlite::Connection;
use std::path::Path;

mod layout;

/// Database layout version this binary reads and writes.
pub const SCHEMA_VERSION: i64 = 4;

/// Header of a new SQLite file, and of stores written before the version header existed.
const UNSTAMPED_VERSION: i64 = 0;

/// Table layout found in a database whose version this binary accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// No schema objects yet: a new file awaiting initialization.
    Empty,
    /// Snapshot, delta operation log, genesis plan and lineage.
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

/// Reject retired, unknown and altered layouts using reads only.
pub(crate) fn check_blocking(connection: &Connection, path: &Path) -> Result<Layout, StoreError> {
    let found = stored_version_blocking(connection, path)?;
    let retired = || StoreError::RetiredSchemaVersion {
        path: path.to_path_buf(),
        found,
        current: SCHEMA_VERSION,
    };
    match found {
        SCHEMA_VERSION => {}
        UNSTAMPED_VERSION => {
            let objects = layout::objects_blocking(connection, path)?;
            if objects.is_empty() {
                return Ok(Layout::Empty);
            }
            // DPM tables under an unstamped header were written before the version header.
            if !layout::has_table(&objects, "plan_state") {
                layout::compare(path, found, &objects, layout::expected())?;
            }
            return Err(retired());
        }
        1..SCHEMA_VERSION => return Err(retired()),
        _ => {
            return Err(StoreError::UnsupportedSchemaVersion {
                path: path.to_path_buf(),
                found,
                supported: SCHEMA_VERSION,
            });
        }
    }
    let objects = layout::objects_blocking(connection, path)?;
    layout::compare(path, found, &objects, layout::expected())?;
    Ok(Layout::Current)
}

/// Create the current tables in an empty database and stamp its version, inside a caller-owned
/// write transaction.
pub(crate) fn create_blocking(connection: &Connection, path: &Path) -> Result<(), StoreError> {
    connection
        .execute_batch(&format!(
            "{}; {}; {}; {}; PRAGMA user_version = {SCHEMA_VERSION};",
            layout::PLAN_STATE,
            layout::OPERATIONS,
            layout::GENESIS,
            layout::LINEAGE
        ))
        .map_err(database_error(path, "initialize schema"))
}

#[cfg(test)]
mod tests;
