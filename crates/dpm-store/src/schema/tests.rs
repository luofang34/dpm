use super::*;
use crate::SqliteStore;
use chrono::Utc;
use dpm_engine::{Command, Operation, apply_command};
use dpm_model::{ActorId, Plan};
use std::path::PathBuf;

/// The layout written by stores created before the version header was introduced.
const BASELINE_DDL: &str = "CREATE TABLE plan_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL,
    plan_json TEXT NOT NULL
);
CREATE TABLE operations (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_id TEXT NOT NULL UNIQUE,
    base_revision INTEGER NOT NULL,
    resulting_revision INTEGER NOT NULL,
    actor_json TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    command_json TEXT NOT NULL
);";

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn claim(plan: &mut Plan) -> Operation {
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    apply_command(
        plan,
        ActorId::agent("owner"),
        Command::Claim { work },
        Utc::now(),
    )
    .expect("claim")
}

fn version(path: &Path) -> i64 {
    let connection = Connection::open(path).expect("raw connection");
    stored_version_blocking(&connection, path).expect("version")
}

/// Append an operation the way a binary without origin support would.
fn raw_append(path: &Path, plan: &Plan, operation: &Operation) {
    let connection = Connection::open(path).expect("raw connection");
    connection
        .execute(
            "INSERT INTO operations(operation_id, base_revision, resulting_revision, actor_json, timestamp, command_json) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                operation.id.to_string(),
                i64::try_from(operation.base_revision).expect("revision"),
                i64::try_from(operation.resulting_revision).expect("revision"),
                serde_json::to_string(&operation.actor).expect("actor"),
                operation.timestamp.to_rfc3339(),
                serde_json::to_string(&operation.command).expect("command"),
            ],
        )
        .expect("operation");
    connection
        .execute(
            "UPDATE plan_state SET revision = ?1, plan_json = ?2",
            rusqlite::params![
                i64::try_from(plan.revision).expect("revision"),
                serde_json::to_string(plan).expect("plan"),
            ],
        )
        .expect("snapshot");
}

fn baseline_store(dir: &Path) -> (PathBuf, Plan) {
    let path = dir.join("baseline.sqlite");
    let plan = fixture();
    let connection = Connection::open(&path).expect("raw connection");
    connection
        .execute_batch(&format!("PRAGMA journal_mode = WAL; {BASELINE_DDL}"))
        .expect("baseline layout");
    connection
        .execute(
            "INSERT INTO plan_state(singleton, revision, plan_json) VALUES(1, 0, ?1)",
            [serde_json::to_string(&plan).expect("json")],
        )
        .expect("baseline snapshot");
    (path, plan)
}

#[test]
fn new_stores_record_the_current_schema_version() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("new.sqlite");
    SqliteStore::open_blocking(&path)
        .expect("store")
        .initialize_blocking(&fixture())
        .expect("initialize");
    assert_eq!(version(&path), SCHEMA_VERSION);
}

#[test]
fn unversioned_baseline_stores_open_read_only_and_are_stamped_by_the_next_write() {
    let dir = tempfile::tempdir().expect("directory");
    let (path, mut plan) = baseline_store(dir.path());
    let mut store = SqliteStore::open_existing_blocking(&path).expect("open baseline");
    assert_eq!(store.load_blocking().expect("load"), Some(plan.clone()));
    assert_eq!(version(&path), 0, "reads must not write");
    let mut stale = plan.clone();
    let mut conflicting = claim(&mut stale);
    conflicting.base_revision = 7;
    conflicting.resulting_revision = 8;
    stale.revision = 8;
    assert!(matches!(
        store.persist_blocking(&stale, &conflicting),
        Err(StoreError::RevisionConflict { .. })
    ));
    assert_eq!(
        version(&path),
        0,
        "the stamp rolls back with a refused write"
    );
    let operation = claim(&mut plan);
    store.persist_blocking(&plan, &operation).expect("persist");
    assert_eq!(version(&path), SCHEMA_VERSION);
    drop(store);
    let reopened = SqliteStore::open_blocking(&path).expect("reopen for writing");
    assert_eq!(reopened.load_blocking().expect("load"), Some(plan));
    assert_eq!(reopened.operation_count_blocking().expect("count"), 1);
}

#[test]
fn opening_a_baseline_store_for_writing_writes_nothing() {
    let dir = tempfile::tempdir().expect("directory");
    let (path, plan) = baseline_store(dir.path());
    let before = std::fs::read(&path).expect("bytes");
    let store = SqliteStore::open_blocking(&path).expect("open baseline");
    assert_eq!(store.load_blocking().expect("load"), Some(plan));
    drop(store);
    assert_eq!(version(&path), 0);
    assert!(std::fs::read(&path).expect("bytes") == before);
}

#[test]
fn the_first_write_records_the_origin_derived_from_existing_history() {
    let dir = tempfile::tempdir().expect("directory");
    let (path, mut plan) = baseline_store(dir.path());
    let first = claim(&mut plan);
    raw_append(&path, &plan, &first);
    let mut store = SqliteStore::open_existing_blocking(&path).expect("open");
    let report = crate::verify_store_blocking(&path).expect("originless verify");
    assert_eq!((report.origin_revision, report.schema_version), (None, 0));
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    let second = apply_command(
        &mut plan,
        ActorId::agent("owner"),
        Command::Block {
            work,
            reason: "waiting".into(),
        },
        Utc::now(),
    )
    .expect("block");
    store.persist_blocking(&plan, &second).expect("persist");
    let report = crate::verify_store_blocking(&path).expect("verify");
    assert_eq!(report.schema_version, SCHEMA_VERSION);
    assert_eq!(report.origin_revision, Some(0));
    assert_eq!(report.operation_count, 2);
}

#[test]
fn an_originless_store_with_a_broken_history_is_never_upgraded() {
    let dir = tempfile::tempdir().expect("directory");
    let (path, mut plan) = baseline_store(dir.path());
    let first = claim(&mut plan);
    raw_append(&path, &plan, &first);
    Connection::open(&path)
        .expect("raw connection")
        .execute_batch("UPDATE operations SET base_revision = 5, resulting_revision = 6")
        .expect("break history");
    let mut store = SqliteStore::open_existing_blocking(&path).expect("open");
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    let second = apply_command(
        &mut plan,
        ActorId::agent("owner"),
        Command::Block {
            work,
            reason: "waiting".into(),
        },
        Utc::now(),
    )
    .expect("block");
    let error = store.persist_blocking(&plan, &second).expect_err("refused");
    assert!(error.is_corruption(), "{error}");
    assert_eq!(version(&path), 0);
    assert_eq!(store.operation_count_blocking().expect("count"), 1);
}

#[test]
fn newer_schema_versions_are_refused_without_any_write() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("future.sqlite");
    SqliteStore::open_blocking(&path)
        .expect("store")
        .initialize_blocking(&fixture())
        .expect("initialize");
    Connection::open(&path)
        .expect("raw connection")
        .execute_batch("PRAGMA journal_mode = DELETE; PRAGMA user_version = 99;")
        .expect("future version");
    let before = std::fs::read(&path).expect("bytes");
    let refused = |result: Result<SqliteStore, StoreError>| {
        assert!(matches!(
            result,
            Err(StoreError::UnsupportedSchemaVersion {
                found: 99,
                supported: SCHEMA_VERSION,
                ..
            })
        ));
    };
    refused(SqliteStore::open_existing_blocking(&path));
    refused(SqliteStore::open_blocking(&path));
    assert!(matches!(
        crate::verify_store_blocking(&path),
        Err(StoreError::UnsupportedSchemaVersion { found: 99, .. })
    ));
    assert_eq!(std::fs::read(&path).expect("bytes"), before);
    assert!(!dir.path().join("future.sqlite-wal").exists());
}

#[test]
fn a_version_raised_by_another_process_after_open_is_never_downgraded() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("raced.sqlite");
    let mut store = SqliteStore::open_blocking(&path).expect("store");
    let mut plan = fixture();
    store.initialize_blocking(&plan).expect("initialize");
    Connection::open(&path)
        .expect("raw connection")
        .execute_batch("PRAGMA user_version = 3")
        .expect("newer binary");
    let operation = claim(&mut plan);
    assert!(matches!(
        store.persist_blocking(&plan, &operation),
        Err(StoreError::UnsupportedSchemaVersion { found: 3, .. })
    ));
    assert_eq!(version(&path), 3);
    let count: i64 = Connection::open(&path)
        .expect("raw connection")
        .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        .expect("count");
    assert_eq!(count, 0);
}

#[test]
fn foreign_sqlite_files_are_refused_and_left_unchanged() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("foreign.sqlite");
    Connection::open(&path)
        .expect("raw connection")
        .execute_batch("CREATE TABLE notes(body TEXT)")
        .expect("foreign table");
    let before = std::fs::read(&path).expect("bytes");
    for result in [
        SqliteStore::open_blocking(&path),
        SqliteStore::open_existing_blocking(&path),
    ] {
        assert!(matches!(result, Err(StoreError::UnrecognizedSchema { .. })));
    }
    assert_eq!(std::fs::read(&path).expect("bytes"), before);
}
