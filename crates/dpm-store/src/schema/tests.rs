use super::*;
use crate::SqliteStore;
use chrono::Utc;
use dpm_engine::{Command, Operation, apply_command};
use dpm_model::{ActorId, Plan};
use std::path::PathBuf;

/// The tables of retired layouts: the snapshot column read by binaries without a version header,
/// and the operation log recording whole proposed plans.
const RETIRED_DDL: &str = "CREATE TABLE plan_state (
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

const RETIRED_ORIGIN_DDL: &str = "CREATE TABLE history_origin (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL
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
        dpm_model::OperationId::new(),
    )
    .expect("claim")
}

fn version(path: &Path) -> i64 {
    let connection = Connection::open(path).expect("raw connection");
    stored_version_blocking(&connection, path).expect("version")
}

/// A store as a binary of the given retired version wrote it, with one whole-plan plan change.
fn retired_store(dir: &Path, version: i64) -> PathBuf {
    let path = dir.join(format!("retired-{version}.sqlite"));
    let connection = Connection::open(&path).expect("raw connection");
    connection
        .execute_batch("PRAGMA journal_mode = DELETE;")
        .expect("rollback journal");
    write_retired(&connection, version);
    path
}

/// A retired WAL store whose writer stopped before any checkpoint: its last commits exist only in
/// the `-wal` file, as after a crash or a binary that never closed cleanly.
fn crashed_wal_store(dir: &Path, version: i64) -> PathBuf {
    let live = dir.join(format!("live-{version}.sqlite"));
    let connection = Connection::open(&live).expect("raw connection");
    connection
        .execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;")
        .expect("wal");
    write_retired(&connection, version);
    let path = dir.join(format!("crashed-{version}.sqlite"));
    for suffix in ["", "-wal", "-shm"] {
        std::fs::copy(
            format!("{}{suffix}", live.display()),
            format!("{}{suffix}", path.display()),
        )
        .expect("copy while the writer is open");
    }
    drop(connection);
    path
}

fn store_files(path: &Path) -> Vec<Option<Vec<u8>>> {
    ["", "-wal", "-shm", "-journal"]
        .iter()
        .map(|suffix| std::fs::read(format!("{}{suffix}", path.display())).ok())
        .collect()
}

fn write_retired(connection: &Connection, version: i64) {
    let plan = fixture();
    let mut renamed = plan.clone();
    renamed.workspace.name = "Renamed".into();
    renamed.revision = 1;
    let origin = if version == 2 { RETIRED_ORIGIN_DDL } else { "" };
    connection
        .execute_batch(&format!(
            "{RETIRED_DDL} {origin} PRAGMA user_version = {version};"
        ))
        .expect("retired layout");
    connection
        .execute(
            "INSERT INTO plan_state(singleton, revision, plan_json) VALUES(1, 1, ?1)",
            [serde_json::to_string(&renamed).expect("json")],
        )
        .expect("snapshot");
    let change = serde_json::json!({"ApplyChange": {"plan": plan, "reason": "rename"}});
    connection
        .execute(
            "INSERT INTO operations(operation_id, base_revision, resulting_revision, actor_json, \
             timestamp, command_json) VALUES('6f1c1a52-7d1e-4d57-9d53-0d3f7a1b2c3d', 0, 1, \
             '{\"kind\":\"Human\",\"name\":\"lead\"}', '2026-01-01T00:00:00+00:00', ?1)",
            [change.to_string()],
        )
        .expect("operation");
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
fn retired_layouts_are_refused_with_guidance_and_left_byte_for_byte_unchanged() {
    let dir = tempfile::tempdir().expect("directory");
    for retired in [0, 1, 2] {
        let path = retired_store(dir.path(), retired);
        let before = std::fs::read(&path).expect("bytes");
        let refusals = [
            SqliteStore::open_blocking(&path).err(),
            SqliteStore::open_existing_blocking(&path).err(),
            crate::verify_store_blocking(&path).err(),
        ];
        for refusal in refusals {
            let Some(StoreError::RetiredSchemaVersion { found, current, .. }) = &refusal else {
                panic!("version {retired}: {refusal:?}");
            };
            assert_eq!((*found, *current), (retired, SCHEMA_VERSION));
            let message = refusal.map(|e| e.to_string()).unwrap_or_default();
            for guidance in ["nothing was written", "archive", "dpm export", "dpm import"] {
                assert!(message.contains(guidance), "{message}");
            }
        }
        assert!(std::fs::read(&path).expect("bytes") == before, "{retired}");
        let side_files = ["-wal", "-shm", "-journal"]
            .map(|suffix| PathBuf::from(format!("{}{suffix}", path.display())));
        assert!(side_files.iter().all(|side| !side.exists()), "{retired}");
    }
}

#[test]
fn retired_wal_stores_with_uncheckpointed_commits_are_refused_without_touching_any_file() {
    let dir = tempfile::tempdir().expect("directory");
    for retired in [0, 1, 2] {
        let path = crashed_wal_store(dir.path(), retired);
        let before = store_files(&path);
        assert!(
            before[1].as_ref().is_some_and(|wal| !wal.is_empty()),
            "the commits are pending in the -wal file"
        );
        for refusal in [
            SqliteStore::open_blocking(&path).err(),
            SqliteStore::open_existing_blocking(&path).err(),
            crate::verify_store_blocking(&path).err(),
        ] {
            assert!(
                matches!(refusal, Some(StoreError::RetiredSchemaVersion { found, .. }) if found == retired),
                "version {retired}: {refusal:?}"
            );
        }
        let after = store_files(&path);
        assert!(
            after[0] == before[0],
            "version {retired}: main file changed"
        );
        assert!(after[1] == before[1], "version {retired}: -wal changed");
        // The -shm file is SQLite's shared-memory index of the WAL, rebuilt by any reader, the
        // release that must export this store included; it holds no data and must only survive.
        assert!(after[2].is_some(), "version {retired}: -shm removed");
        assert!(after[3].is_none(), "version {retired}: journal created");
    }
}

#[test]
fn binaries_without_the_version_header_cannot_read_the_current_snapshot() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("current.sqlite");
    SqliteStore::open_blocking(&path)
        .expect("store")
        .initialize_blocking(&fixture())
        .expect("initialize");
    let connection = Connection::open(&path).expect("raw connection");
    // Those binaries create their tables if missing and then read this column.
    connection
        .execute_batch(&RETIRED_DDL.replace("CREATE TABLE", "CREATE TABLE IF NOT EXISTS"))
        .expect("no-op on existing tables");
    let read = connection.query_row("SELECT revision, plan_json FROM plan_state", [], |row| {
        row.get::<_, i64>(0)
    });
    assert!(read.is_err(), "{read:?}");
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
    let newer = SCHEMA_VERSION + 1;
    Connection::open(&path)
        .expect("raw connection")
        .execute_batch(&format!("PRAGMA user_version = {newer}"))
        .expect("newer binary");
    let operation = claim(&mut plan);
    assert!(matches!(
        store.persist_blocking(&plan, &operation),
        Err(StoreError::UnsupportedSchemaVersion { found, .. }) if found == newer
    ));
    assert_eq!(version(&path), newer);
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
