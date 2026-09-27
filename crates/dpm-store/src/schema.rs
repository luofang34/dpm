//! Database layout version stored in SQLite's `user_version` header field.
//!
//! Policy: version 1 is the `plan_state` + `operations` layout. A store whose header still reads 0
//! but has exactly that layout is the unversioned baseline written before versioning existed; it is
//! read as version 1 and stamped by the next write transaction, so read-only commands never write.
//! A newer version is refused before any statement that could write. A future layout change adds a
//! migration that runs in one IMMEDIATE transaction when the store is opened for writing; an older
//! version without a registered migration is refused with instructions instead of being guessed at.

use crate::{StoreError, error::database_error};
use rusqlite::Connection;
use std::path::Path;

/// Database layout version this binary reads and writes.
pub const SCHEMA_VERSION: i64 = 1;

const BASELINE_VERSION: i64 = 0;
const PLAN_STATE_COLUMNS: [&str; 3] = ["singleton", "revision", "plan_json"];
const OPERATION_COLUMNS: [&str; 7] = [
    "sequence",
    "operation_id",
    "base_revision",
    "resulting_revision",
    "actor_json",
    "timestamp",
    "command_json",
];
const CREATE_TABLES: &str = "CREATE TABLE IF NOT EXISTS plan_state (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 revision INTEGER NOT NULL,
                 plan_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS operations (
                 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                 operation_id TEXT NOT NULL UNIQUE,
                 base_revision INTEGER NOT NULL,
                 resulting_revision INTEGER NOT NULL,
                 actor_json TEXT NOT NULL,
                 timestamp TEXT NOT NULL,
                 command_json TEXT NOT NULL
             );";

/// Table layout found in a database whose version this binary accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// No tables yet: a new file awaiting creation.
    Empty,
    /// The DPM snapshot and operation log tables.
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

/// Reject unknown versions and foreign layouts using reads only.
pub(crate) fn check_blocking(connection: &Connection, path: &Path) -> Result<Layout, StoreError> {
    let found = stored_version_blocking(connection, path)?;
    if !(BASELINE_VERSION..=SCHEMA_VERSION).contains(&found) {
        return Err(StoreError::UnsupportedSchemaVersion {
            path: path.to_path_buf(),
            found,
            supported: SCHEMA_VERSION,
        });
    }
    let layout = layout_blocking(connection, path)?;
    if found == SCHEMA_VERSION && layout == Layout::Empty {
        return Err(unrecognized(path, "schema version 1 without DPM tables"));
    }
    Ok(layout)
}

/// Create the tables of an empty database and stamp its version in one IMMEDIATE transaction.
pub(crate) fn create_blocking(connection: &mut Connection, path: &Path) -> Result<(), StoreError> {
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database_error(path, "begin schema creation"))?;
    // A concurrent creator may have committed between the unlocked check and this lock.
    if check_blocking(&transaction, path)? == Layout::Empty {
        transaction
            .execute_batch(CREATE_TABLES)
            .map_err(database_error(path, "initialize schema"))?;
    }
    stamp_blocking(&transaction, path)?;
    transaction
        .commit()
        .map_err(database_error(path, "commit schema creation"))
}

/// Record the current version inside a caller-owned write transaction, so it commits or rolls
/// back together with that write. A version changed by another process since open is re-checked
/// here, so a newer store is never downgraded.
pub(crate) fn stamp_blocking(connection: &Connection, path: &Path) -> Result<(), StoreError> {
    match stored_version_blocking(connection, path)? {
        SCHEMA_VERSION => Ok(()),
        BASELINE_VERSION => connection
            .execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
            .map_err(database_error(path, "stamp schema version")),
        found => Err(StoreError::UnsupportedSchemaVersion {
            path: path.to_path_buf(),
            found,
            supported: SCHEMA_VERSION,
        }),
    }
}

fn layout_blocking(connection: &Connection, path: &Path) -> Result<Layout, StoreError> {
    let tables = names_blocking(
        connection,
        path,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        [],
    )?;
    if tables.is_empty() {
        return Ok(Layout::Empty);
    }
    if tables != ["operations", "plan_state"] {
        return Err(unrecognized(
            path,
            &format!("unexpected tables {}", tables.join(", ")),
        ));
    }
    for (table, expected) in [
        ("plan_state", &PLAN_STATE_COLUMNS[..]),
        ("operations", &OPERATION_COLUMNS[..]),
    ] {
        let columns = names_blocking(
            connection,
            path,
            "SELECT name FROM pragma_table_info(?1) ORDER BY cid",
            [table],
        )?;
        if columns != expected {
            return Err(unrecognized(
                path,
                &format!("table {table} has columns {}", columns.join(", ")),
            ));
        }
    }
    Ok(Layout::Current)
}

fn names_blocking<P: rusqlite::Params>(
    connection: &Connection,
    path: &Path,
    sql: &str,
    params: P,
) -> Result<Vec<String>, StoreError> {
    let mut statement = connection
        .prepare(sql)
        .map_err(database_error(path, "prepare schema query"))?;
    let names = statement
        .query_map(params, |row| row.get(0))
        .map_err(database_error(path, "inspect schema"))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(database_error(path, "read schema"))?;
    Ok(names)
}

fn unrecognized(path: &Path, detail: &str) -> StoreError {
    StoreError::UnrecognizedSchema {
        path: path.to_path_buf(),
        detail: detail.to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
