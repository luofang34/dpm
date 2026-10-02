//! The exact layout of a run store, versioned independently of the project store.
//!
//! A run store is a sidecar file: it never changes the project store's layout, replay or writer
//! lock. Like the project store it names one exact layout per accepted `user_version`, compared
//! object by object, so a copied file with a planted trigger or a foreign layout is refused before
//! any statement runs. Version 1 is the only layout this binary reads or writes; any other version
//! is refused unchanged.

use crate::{
    StoreError,
    error::database_error,
    schema::{
        Layout,
        layout::{SchemaObject, compare, objects_blocking},
    },
};
use rusqlite::Connection;
use std::path::Path;

/// Layout version of the run store this binary reads and writes.
pub const RUN_STORE_VERSION: i64 = 1;

const UNSTAMPED_VERSION: i64 = 0;

/// The workspace and history the store was created for. `archived` marks a backup that is read and
/// verified but never written until restored.
const BINDING: &str = "CREATE TABLE run_binding (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 workspace_id TEXT NOT NULL,
                 lineage_id TEXT NOT NULL,
                 archived INTEGER NOT NULL CHECK (archived IN (0, 1))
             )";
/// One immutable row per run: the start record as typed JSON, with the task it executes.
const RUNS: &str = "CREATE TABLE runs (
                 run_id TEXT PRIMARY KEY,
                 work_id TEXT NOT NULL,
                 record_json TEXT NOT NULL
             )";
/// Durable lifecycle, never pruned. `event_id` is the idempotency key; a run's start is its first
/// row, carrying the run's own identity.
const LIFECYCLE: &str = "CREATE TABLE run_lifecycle (
                 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                 event_id TEXT NOT NULL UNIQUE,
                 run_id TEXT NOT NULL REFERENCES runs(run_id),
                 state TEXT NOT NULL,
                 detail TEXT,
                 observed_at TEXT,
                 recorded_by_json TEXT NOT NULL,
                 recorded_at TEXT NOT NULL
             )";
const LIFECYCLE_BY_RUN: &str =
    "CREATE INDEX run_lifecycle_by_run ON run_lifecycle(run_id, sequence)";
/// Bounded activity with its own cursor. `AUTOINCREMENT` keeps a pruned sequence from ever being
/// assigned again, so a stale cursor can be recognised.
const ACTIVITY: &str = "CREATE TABLE run_activity (
                 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                 run_id TEXT NOT NULL REFERENCES runs(run_id),
                 source_sequence INTEGER NOT NULL,
                 kind TEXT NOT NULL,
                 text TEXT,
                 truncated INTEGER NOT NULL CHECK (truncated IN (0, 1)),
                 text_digest TEXT,
                 observed_at TEXT,
                 recorded_by_json TEXT NOT NULL,
                 recorded_at TEXT NOT NULL,
                 UNIQUE (run_id, source_sequence)
             )";
const ACTIVITY_BY_RUN: &str = "CREATE INDEX run_activity_by_run ON run_activity(run_id, sequence)";
/// What each run's activity amounts to, so retention cannot erase the fact of its silence.
const TALLY: &str = "CREATE TABLE run_activity_tally (
                 run_id TEXT PRIMARY KEY REFERENCES runs(run_id),
                 recorded INTEGER NOT NULL,
                 last_kind TEXT NOT NULL,
                 high_water INTEGER NOT NULL,
                 last_at TEXT NOT NULL
             )";
/// Every activity sequence up to and including `pruned_through` has been removed by retention.
const RETENTION: &str = "CREATE TABLE run_activity_retention (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 pruned_through INTEGER NOT NULL
             )";
/// Explicit associations of project operations with runs; an operation links to at most one run.
const LINKS: &str = "CREATE TABLE run_links (
                 operation_id TEXT PRIMARY KEY,
                 run_id TEXT NOT NULL REFERENCES runs(run_id),
                 work_id TEXT NOT NULL,
                 linked_by_json TEXT NOT NULL,
                 linked_at TEXT NOT NULL
             )";
/// Where each lineage a restore created forked from: the lineage it continued and the project
/// revision it started at. A restore adds one row, so a lineage with no project operations of its
/// own can still be traced back to the history that holds the revision its runs observed.
const FORKS: &str = "CREATE TABLE run_lineage_forks (
                 lineage_id TEXT PRIMARY KEY,
                 parent_lineage_id TEXT NOT NULL,
                 revision INTEGER NOT NULL
             )";
/// SQLite creates this table itself for the `AUTOINCREMENT` columns.
const SEQUENCE: &str = "CREATE TABLE sqlite_sequence(name,seq)";

fn expected() -> Vec<SchemaObject> {
    let table = |name: &str, sql| SchemaObject::new("table", name, name, Some(sql));
    let index = |name: &str, on: &str, sql| SchemaObject::new("index", name, on, Some(sql));
    let automatic =
        |on: &str| SchemaObject::new("index", &format!("sqlite_autoindex_{on}_1"), on, None);
    let mut objects = vec![
        table("run_binding", BINDING),
        table("runs", RUNS),
        table("run_lifecycle", LIFECYCLE),
        table("run_activity", ACTIVITY),
        table("run_activity_tally", TALLY),
        table("run_activity_retention", RETENTION),
        table("run_links", LINKS),
        table("run_lineage_forks", FORKS),
        table("sqlite_sequence", SEQUENCE),
        index("run_lifecycle_by_run", "run_lifecycle", LIFECYCLE_BY_RUN),
        index("run_activity_by_run", "run_activity", ACTIVITY_BY_RUN),
        automatic("runs"),
        automatic("run_lifecycle"),
        automatic("run_activity"),
        automatic("run_activity_tally"),
        automatic("run_links"),
        automatic("run_lineage_forks"),
    ];
    objects.sort();
    objects
}

/// Reject unknown, newer and altered layouts using reads only.
pub(super) fn check_blocking(connection: &Connection, path: &Path) -> Result<Layout, StoreError> {
    let found: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(database_error(path, "read run store version"))?;
    let objects = objects_blocking(connection, path)?;
    match found {
        UNSTAMPED_VERSION if objects.is_empty() => Ok(Layout::Empty),
        UNSTAMPED_VERSION => Err(StoreError::UnrecognizedSchema {
            path: path.to_path_buf(),
            detail: "not a DPM run store: it has tables but no run store version".into(),
        }),
        RUN_STORE_VERSION => {
            compare(path, found, &objects, expected())?;
            Ok(Layout::Current)
        }
        _ => Err(StoreError::UnsupportedSchemaVersion {
            path: path.to_path_buf(),
            found,
            supported: RUN_STORE_VERSION,
        }),
    }
}

/// Create the layout in an empty database inside a caller-owned write transaction and stamp it.
pub(super) fn create_blocking(connection: &Connection, path: &Path) -> Result<(), StoreError> {
    connection
        .execute_batch(&format!(
            "{BINDING}; {RUNS}; {LIFECYCLE}; {LIFECYCLE_BY_RUN}; {ACTIVITY}; {ACTIVITY_BY_RUN}; \
             {TALLY}; {RETENTION}; {LINKS}; {FORKS}; PRAGMA user_version = {RUN_STORE_VERSION};"
        ))
        .map_err(database_error(path, "initialize run store schema"))
}
