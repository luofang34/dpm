use super::*;
use chrono::Utc;
use dpm_engine::{Command, apply_command};
use dpm_model::{ActorId, Plan};
use rusqlite::types::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

/// Claim, then alternate block/unblock so a writer can append operations indefinitely.
fn next_command(plan: &Plan, step: u64) -> Command {
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    match step {
        0 => Command::Claim { work },
        n if n % 2 == 1 => Command::Block {
            work,
            reason: format!("waiting {n}"),
        },
        _ => Command::Unblock { work },
    }
}

fn append(store: &mut SqliteStore, step: u64) {
    let mut plan = store.load_blocking().expect("load").expect("plan");
    let command = next_command(&plan, step);
    let operation = apply_command(
        &mut plan,
        ActorId::agent("owner"),
        command,
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("command");
    store.persist_blocking(&plan, &operation).expect("persist");
}

fn store_with_history(path: &Path, operations: u64) -> SqliteStore {
    let mut store = SqliteStore::open_blocking(path).expect("store");
    store.initialize_blocking(&fixture()).expect("initialize");
    for step in 0..operations {
        append(&mut store, step);
    }
    store
}

fn rows(path: &Path, sql: &str) -> Vec<Vec<Value>> {
    let connection = Connection::open(path).expect("raw connection");
    let mut statement = connection.prepare(sql).expect("prepare");
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|index| row.get::<_, Value>(index))
                .collect()
        })
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

fn journal_mode(path: &Path) -> Vec<Vec<Value>> {
    rows(path, "PRAGMA journal_mode")
}

#[test]
fn backups_taken_while_another_connection_writes_are_consistent() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("live.sqlite");
    let store = store_with_history(&path, 1);
    let stop = Arc::new(AtomicBool::new(false));
    let (committed, commits) = mpsc::channel();
    let writer = {
        let (path, stop) = (path.clone(), Arc::clone(&stop));
        std::thread::spawn(move || {
            let mut writer = SqliteStore::open_existing_blocking(&path).expect("writer");
            let mut step = 1;
            while !stop.load(Ordering::SeqCst) {
                append(&mut writer, step);
                step += 1;
                if committed.send(step).is_err() {
                    break;
                }
            }
        })
    };
    let mut previous = 0;
    for index in 0..8 {
        commits.recv().expect("a concurrent commit");
        let target = dir.path().join(format!("backup-{index}.sqlite"));
        let report = store.backup_blocking(&target).expect("backup");
        assert!(report.operation_count >= previous);
        assert_eq!(verify_store_blocking(&target).expect("reverify"), report);
        previous = report.operation_count;
    }
    stop.store(true, Ordering::SeqCst);
    drop(commits);
    writer.join().expect("writer thread");
    assert!(previous > 1, "backups observed concurrent operations");
}

#[test]
fn restore_round_trip_preserves_snapshot_history_and_version_exactly() {
    let dir = tempfile::tempdir().expect("directory");
    let live = dir.path().join("live.sqlite");
    let backup = dir.path().join("backup.sqlite");
    let restored = dir.path().join("restored/state.sqlite");
    std::fs::create_dir(dir.path().join("restored")).expect("directory");
    let store = store_with_history(&live, 5);
    let backed_up = store.backup_blocking(&backup).expect("backup");
    assert_eq!(backed_up.operation_count, 5);
    assert_eq!(backed_up.schema_version, crate::SCHEMA_VERSION);
    assert_eq!(journal_mode(&backup), [[Value::Text("delete".into())]]);
    assert!(!dir.path().join("backup.sqlite-wal").exists());
    let report = restore_store_blocking(&backup, &restored).expect("restore");
    assert_eq!(
        report.path,
        std::fs::canonicalize(&restored).expect("canonical")
    );
    assert_eq!(
        (report.workspace_id, report.revision),
        (backed_up.workspace_id, backed_up.revision)
    );
    assert_eq!(journal_mode(&restored), [[Value::Text("wal".into())]]);
    for sql in [
        "SELECT revision, snapshot_json FROM plan_state",
        "SELECT * FROM operations ORDER BY sequence",
        "PRAGMA user_version",
    ] {
        assert_eq!(rows(&restored, sql), rows(&live, sql), "{sql}");
    }
    let mut reopened = SqliteStore::open_existing_blocking(&restored).expect("reopen");
    append(&mut reopened, 5);
    assert_eq!(
        verify_store_blocking(&restored)
            .expect("verify")
            .operation_count,
        6
    );
}

#[test]
fn backup_and_restore_never_overwrite_existing_files() {
    let dir = tempfile::tempdir().expect("directory");
    let live = dir.path().join("live.sqlite");
    let backup = dir.path().join("backup.sqlite");
    let store = store_with_history(&live, 2);
    std::fs::write(&backup, b"keep me").expect("existing file");
    assert!(matches!(
        store.backup_blocking(&backup),
        Err(StoreError::TargetExists { path }) if *path == std::fs::canonicalize(&backup).expect("canonical")
    ));
    assert_eq!(std::fs::read(&backup).expect("bytes"), b"keep me");
    std::fs::remove_file(&backup).expect("remove");
    store.backup_blocking(&backup).expect("backup");
    let live_rows = rows(&live, "SELECT * FROM operations");
    assert!(matches!(
        restore_store_blocking(&backup, &live),
        Err(StoreError::TargetExists { .. })
    ));
    assert_eq!(rows(&live, "SELECT * FROM operations"), live_rows);
    let fresh = dir.path().join("fresh.sqlite");
    let stale_wal = dir.path().join("fresh.sqlite-wal");
    std::fs::write(&stale_wal, b"stale").expect("stale side file");
    assert!(matches!(
        restore_store_blocking(&backup, &fresh),
        Err(StoreError::TargetExists { path }) if *path == std::fs::canonicalize(&stale_wal).expect("canonical")
    ));
    assert!(!fresh.exists());
}

#[test]
fn truncated_and_corrupted_backups_are_detected_and_never_restored() {
    let dir = tempfile::tempdir().expect("directory");
    let backup = dir.path().join("backup.sqlite");
    store_with_history(&dir.path().join("live.sqlite"), 4)
        .backup_blocking(&backup)
        .expect("backup");
    let bytes = std::fs::read(&backup).expect("bytes");
    let truncated = dir.path().join("truncated.sqlite");
    std::fs::write(&truncated, &bytes[..bytes.len() / 2]).expect("truncate");
    // Damage the live b-tree page of the history; free pages would pass the page check.
    let root = rows(
        &backup,
        "SELECT rootpage * 1, (SELECT page_size FROM pragma_page_size) FROM sqlite_master WHERE name = 'operations'",
    );
    let [Value::Integer(page), Value::Integer(size)] = root[0][..] else {
        panic!("operations root page: {root:?}");
    };
    let start = usize::try_from((page - 1) * size).expect("offset");
    let mut damaged = bytes.clone();
    damaged[start..start + 64].fill(0xA5);
    let corrupted = dir.path().join("corrupted.sqlite");
    std::fs::write(&corrupted, damaged).expect("corrupt");
    for source in [&truncated, &corrupted] {
        let error = verify_store_blocking(source).expect_err("damage detected");
        assert!(error.is_corruption(), "{source:?}: {error}");
        let target = dir.path().join("restored.sqlite");
        assert!(restore_store_blocking(source, &target).is_err());
        assert!(!target.exists(), "a refused restore leaves no target");
    }
}

#[test]
fn history_gaps_and_stale_snapshots_fail_verification() {
    let dir = tempfile::tempdir().expect("directory");
    let gap = dir.path().join("gap.sqlite");
    let stale = dir.path().join("stale.sqlite");
    let store = store_with_history(&dir.path().join("live.sqlite"), 3);
    store.backup_blocking(&gap).expect("backup");
    store.backup_blocking(&stale).expect("backup");
    let edit = |path: &Path, sql: &str| {
        Connection::open(path)
            .expect("raw connection")
            .execute_batch(sql)
            .expect("tamper");
    };
    edit(&gap, "DELETE FROM operations WHERE sequence = 2");
    assert!(matches!(
        verify_store_blocking(&gap),
        Err(StoreError::SequenceGap {
            previous: 1,
            sequence: 3,
            ..
        })
    ));
    let broken = dir.path().join("broken.sqlite");
    store.backup_blocking(&broken).expect("backup");
    edit(
        &broken,
        "UPDATE operations SET base_revision = 7, resulting_revision = 8 WHERE sequence = 2",
    );
    assert!(matches!(
        verify_store_blocking(&broken),
        Err(StoreError::HistoryDiscontinuity {
            sequence: 2,
            expected: 1,
            found: 7,
            ..
        })
    ));
    edit(&stale, "DELETE FROM operations WHERE sequence = 3");
    assert!(matches!(
        verify_store_blocking(&stale),
        Err(StoreError::SnapshotRevisionMismatch {
            snapshot: 3,
            last_operation: 2,
            ..
        })
    ));
}

#[test]
fn verification_is_read_only() {
    let dir = tempfile::tempdir().expect("directory");
    let backup = dir.path().join("backup.sqlite");
    store_with_history(&dir.path().join("live.sqlite"), 2)
        .backup_blocking(&backup)
        .expect("backup");
    let before = std::fs::read(&backup).expect("bytes");
    let report = verify_store_blocking(&backup).expect("verify");
    assert_eq!((report.revision, report.operation_count), (2, 2));
    assert_eq!(report.genesis_revision, 0);
    assert_eq!(std::fs::read(&backup).expect("bytes"), before);
}

mod golden;
mod replay_history;
mod tampering;
