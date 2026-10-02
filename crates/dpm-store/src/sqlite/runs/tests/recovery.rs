//! Refusals that leave files unchanged, tamper detection, and the backup, restore and
//! verification policy that carries runs beside the project store.

use super::*;
use crate::{
    SqliteStore, restore_store_blocking, sqlite::runs::recovery::verify_blocking,
    verify_store_blocking,
};
use std::{fs, path::Path};

fn populated(path: &Path) -> (RunStore, RunRecord, Plan) {
    let (plan, work) = claimed();
    let lineage = LineageId::new();
    let mut store =
        RunStore::open_or_create_blocking(path, plan.workspace.id, lineage).expect("create");
    let record = record(&plan, lineage, &request(work));
    store.start_blocking(&record, lineage).expect("start");
    let activity = |source_sequence: u64| ActivityInput {
        run: record.id,
        source_sequence,
        kind: ActivityKind::Progress,
        text: None,
        observed_at: None,
    };
    store
        .append_activity_blocking(&[activity(1), activity(2)], &worker(), Utc::now(), lineage)
        .expect("activity");
    (store, record, plan)
}

/// Every file in a directory with its bytes, to prove a refusal changed nothing.
fn files(directory: &Path) -> Vec<(String, Vec<u8>)> {
    let mut found: Vec<_> = fs::read_dir(directory)
        .expect("directory")
        .map(|entry| {
            let entry = entry.expect("entry");
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).expect("bytes"),
            )
        })
        .collect();
    found.sort();
    found
}

fn raw(path: &Path) -> Connection {
    Connection::open(path).expect("raw connection")
}

#[test]
fn reading_a_run_store_that_does_not_exist_creates_nothing() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("plan.sqlite.runs");
    let opened = RunStore::open_existing_blocking(&path, WorkspaceId::new()).expect("open");
    assert!(opened.is_none());
    assert!(files(directory.path()).is_empty());
}

#[test]
fn an_empty_file_left_by_an_interrupted_creation_is_not_a_store_and_can_be_initialized() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("plan.sqlite.runs");
    fs::write(&path, b"").expect("empty file");
    let (plan, _) = claimed();
    assert!(
        RunStore::open_existing_blocking(&path, plan.workspace.id)
            .expect("open")
            .is_none()
    );
    RunStore::open_or_create_blocking(&path, plan.workspace.id, LineageId::new()).expect("create");
    let reopened = RunStore::open_existing_blocking(&path, plan.workspace.id).expect("open");
    assert!(reopened.is_some());
}

#[test]
fn another_version_a_foreign_layout_and_a_planted_trigger_are_refused_unchanged() {
    let directory = tempfile::tempdir().expect("directory");
    let (plan, _) = claimed();
    for (name, damage) in [
        ("newer", "PRAGMA user_version = 99"),
        ("foreign", "CREATE TABLE intruder (x)"),
        (
            "trigger",
            "CREATE TRIGGER spy AFTER INSERT ON run_lifecycle BEGIN SELECT 1; END",
        ),
    ] {
        let path = directory.path().join(format!("{name}.sqlite.runs"));
        let (store, _, _) = populated(&path);
        drop(store);
        raw(&path).execute_batch(damage).expect("damage");
        let before = files(directory.path());
        let refused = RunStore::open_existing_blocking(&path, plan.workspace.id);
        assert!(
            matches!(
                refused,
                Err(RunStoreError::Store(
                    StoreError::UnsupportedSchemaVersion { .. }
                        | StoreError::UnrecognizedSchema { .. }
                ))
            ),
            "{name}: {:?}",
            refused.err()
        );
        assert_eq!(files(directory.path()), before, "{name} was modified");
    }
}

#[test]
fn a_run_store_bound_to_another_workspace_is_refused_unchanged() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("plan.sqlite.runs");
    let (store, _, _) = populated(&path);
    drop(store);
    let before = files(directory.path());
    let refused = RunStore::open_existing_blocking(&path, WorkspaceId::new());
    assert!(matches!(
        refused,
        Err(RunStoreError::WorkspaceMismatch { .. })
    ));
    assert!(matches!(
        RunStore::open_or_create_blocking(&path, WorkspaceId::new(), LineageId::new()),
        Err(RunStoreError::WorkspaceMismatch { .. })
    ));
    assert_eq!(files(directory.path()), before);
}

#[test]
fn verification_counts_what_is_held_and_reads_the_file_without_writing() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("plan.sqlite.runs");
    let (store, record, plan) = populated(&path);
    drop(store);
    let before = files(directory.path());
    let report = verify_blocking(&path).expect("verified");
    assert_eq!(report.workspace_id, plan.workspace.id);
    assert_eq!(report.lineage_id, record.contract.lineage_id);
    assert_eq!(
        (
            report.runs,
            report.lifecycle_events,
            report.activity_retained,
            report.activity_pruned_through
        ),
        (1, 1, 2, 0)
    );
    assert_eq!(report.schema_version, RUN_STORE_VERSION);
    assert!(!report.archived);
    assert_eq!(files(directory.path()), before, "verification wrote");
}

#[test]
fn verification_reports_removed_reordered_and_forged_records() {
    let cases: [(&str, &str); 5] = [
        ("lifecycle row removed", "DELETE FROM run_lifecycle"),
        (
            "activity removed from the middle",
            "DELETE FROM run_activity WHERE source_sequence = 1",
        ),
        (
            "state after the end",
            "INSERT INTO run_lifecycle(event_id, run_id, state, recorded_by_json, recorded_at) \
             SELECT '0192f000-0000-7000-8000-000000000001', run_id, 'completed', \
             '{\"kind\":\"Agent\",\"name\":\"worker\"}', '2026-01-01T00:00:00+00:00' FROM runs; \
             INSERT INTO run_lifecycle(event_id, run_id, state, recorded_by_json, recorded_at) \
             SELECT '0192f000-0000-7000-8000-000000000002', run_id, 'working', \
             '{\"kind\":\"Agent\",\"name\":\"worker\"}', '2026-01-01T00:00:01+00:00' FROM runs",
        ),
        (
            "undecodable record",
            "UPDATE runs SET record_json = '{\"id\": 1}'",
        ),
        (
            "a high-water mark below a retained record",
            "UPDATE run_activity_tally SET high_water = 0",
        ),
    ];
    for (name, damage) in cases {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("plan.sqlite.runs");
        let (store, _, _) = populated(&path);
        drop(store);
        raw(&path).execute_batch(damage).expect("damage");
        let error = verify_blocking(&path).expect_err(name);
        assert!(error.is_corruption(), "{name}: {error}");
    }
}

fn project(directory: &Path) -> (SqliteStore, std::path::PathBuf, Plan, WorkItemId) {
    let (plan, work) = claimed();
    let path = directory.join("plan.sqlite");
    let mut store = SqliteStore::open_blocking(&path).expect("project store");
    store.initialize_blocking(&plan).expect("initialize");
    (store, path, plan, work)
}

fn run_beside(
    store: &SqliteStore,
    plan: &Plan,
    work: WorkItemId,
) -> (RunStore, RunRecord, LineageId) {
    let lineage = store
        .lineage_blocking()
        .expect("lineage")
        .expect("lineage")
        .lineage_id;
    let mut runs = RunStore::open_or_create_blocking(
        &RunStore::sidecar_path(store.path()),
        plan.workspace.id,
        lineage,
    )
    .expect("run store");
    let record = record(plan, lineage, &request(work));
    runs.start_blocking(&record, lineage).expect("start");
    (runs, record, lineage)
}

#[test]
fn a_project_backup_carries_the_run_store_and_says_so() {
    let directory = tempfile::tempdir().expect("directory");
    let (store, _, plan, work) = project(directory.path());
    let (_runs, record, lineage) = run_beside(&store, &plan, work);
    let backup = directory.path().join("backup.sqlite");
    let report = store.backup_blocking(&backup).expect("backup");
    let runs = report
        .runs
        .clone()
        .expect("the report names the run store it carried");
    assert_eq!((runs.runs, runs.lifecycle_events), (1, 1));
    assert!(runs.archived);
    assert_eq!(runs.lineage_id, lineage);
    assert!(RunStore::sidecar_path(&backup).exists());
    let json = serde_json::to_value(&report).expect("json");
    assert!(json.get("runs").is_some());
    // Verifying the archive verifies its runs with it.
    let verified = verify_store_blocking(&backup).expect("verify");
    assert_eq!(verified.runs.expect("runs").runs, 1);
    // The archive's run store is never written.
    let mut archived =
        RunStore::open_existing_blocking(&RunStore::sidecar_path(&backup), plan.workspace.id)
            .expect("open")
            .expect("exists");
    assert!(matches!(
        archived.start_blocking(&record, lineage),
        Err(RunStoreError::Store(StoreError::Lineage(
            LineageError::Archived { .. }
        )))
    ));
}

#[test]
fn a_store_without_runs_backs_up_without_a_run_store() {
    let directory = tempfile::tempdir().expect("directory");
    let (store, _, _, _) = project(directory.path());
    let backup = directory.path().join("backup.sqlite");
    let report = store.backup_blocking(&backup).expect("backup");
    assert!(report.runs.is_none());
    assert!(!RunStore::sidecar_path(&backup).exists());
    assert!(
        serde_json::to_value(&report)
            .expect("json")
            .get("runs")
            .is_none()
    );
}

#[test]
fn a_restore_forks_the_run_store_to_the_new_lineage_and_keeps_old_attribution() {
    let directory = tempfile::tempdir().expect("directory");
    let (store, _, plan, work) = project(directory.path());
    let (_runs, record, original) = run_beside(&store, &plan, work);
    let backup = directory.path().join("backup.sqlite");
    store.backup_blocking(&backup).expect("backup");
    let restored = directory.path().join("restored.sqlite");
    let report = restore_store_blocking(&backup, &restored).expect("restore");
    assert_ne!(
        report.lineage_id, original,
        "a restore starts a new history"
    );
    let runs = report.runs.clone().expect("runs restored with the store");
    assert_eq!(
        runs.lineage_id, report.lineage_id,
        "the run store follows the new lineage"
    );
    assert!(!runs.archived);
    let reopened =
        RunStore::open_existing_blocking(&RunStore::sidecar_path(&restored), plan.workspace.id)
            .expect("open")
            .expect("exists");
    let snapshot = reopened
        .snapshot_blocking(record.id)
        .expect("read")
        .expect("run");
    assert_eq!(
        snapshot.record.contract.lineage_id, original,
        "historical attribution is not rewritten"
    );
    // The restored store accepts new runs under its own lineage, beside the foreign ones.
    let mut reopened = reopened;
    let fresh = super::record(&plan, report.lineage_id, &request(work));
    reopened
        .start_blocking(&fresh, report.lineage_id)
        .expect("new run");
    assert_eq!(
        reopened.snapshots_blocking(None, 10).expect("runs").len(),
        2
    );
    // The live store's run store was not touched by any of this.
    assert!(
        verify_store_blocking(store.path())
            .expect("live")
            .runs
            .is_some()
    );
}

#[test]
fn a_restore_without_runs_does_not_adopt_a_run_store_already_at_the_target() {
    let directory = tempfile::tempdir().expect("directory");
    let (store, _, _, _) = project(directory.path());
    let backup = directory.path().join("backup.sqlite");
    store.backup_blocking(&backup).expect("backup");
    let restored = directory.path().join("restored.sqlite");
    let stray = RunStore::sidecar_path(&restored);
    fs::write(&stray, b"not ours").expect("stray file");
    assert!(matches!(
        restore_store_blocking(&backup, &restored),
        Err(StoreError::TargetExists { .. })
    ));
    assert!(!restored.exists(), "nothing was created");
    assert_eq!(fs::read(&stray).expect("stray"), b"not ours");
}

#[test]
fn a_restore_whose_run_archive_is_damaged_leaves_no_target_behind() {
    let directory = tempfile::tempdir().expect("directory");
    let (store, _, plan, work) = project(directory.path());
    run_beside(&store, &plan, work);
    let backup = directory.path().join("backup.sqlite");
    store.backup_blocking(&backup).expect("backup");
    raw(&RunStore::sidecar_path(&backup))
        .execute_batch("DELETE FROM run_lifecycle")
        .expect("damage");
    let restored = directory.path().join("restored.sqlite");
    let error = restore_store_blocking(&backup, &restored).expect_err("damaged archive");
    assert!(error.is_corruption(), "{error}");
    assert!(!restored.exists());
    assert!(!RunStore::sidecar_path(&restored).exists());
}

#[test]
fn a_live_run_store_is_not_a_restore_source() {
    let directory = tempfile::tempdir().expect("directory");
    let (store, path, plan, work) = project(directory.path());
    run_beside(&store, &plan, work);
    let restored = directory.path().join("restored.sqlite");
    assert!(restore_store_blocking(&path, &restored).is_err());
    assert!(!restored.exists());
}

#[test]
fn run_writes_and_project_writes_do_not_wait_for_each_other() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut store, path, plan, work) = project(directory.path());
    let (mut runs, record, lineage) = run_beside(&store, &plan, work);
    store
        .set_busy_timeout_blocking(std::time::Duration::from_millis(50))
        .expect("timeout");
    // A project writer holding the project store's write lock does not block run activity.
    let holder = raw(&path);
    holder
        .execute_batch("BEGIN IMMEDIATE")
        .expect("hold the project writer lock");
    runs.append_activity_blocking(
        &[ActivityInput {
            run: record.id,
            source_sequence: 1,
            kind: ActivityKind::Heartbeat,
            text: None,
            observed_at: None,
        }],
        &worker(),
        Utc::now(),
        lineage,
    )
    .expect("run write proceeds");
    holder.execute_batch("ROLLBACK").expect("release");
    // And a run writer holding the run store's lock does not block a project commit.
    let run_holder = raw(&RunStore::sidecar_path(&path));
    run_holder
        .execute_batch("BEGIN IMMEDIATE")
        .expect("hold the run writer lock");
    let mut next = plan.clone();
    let operation = apply_command(
        &mut next,
        worker(),
        Command::Start {
            work,
            occurred_at: None,
        },
        Utc::now(),
        OperationId::new(),
    )
    .expect("start");
    store
        .persist_blocking(&next, &operation, None)
        .expect("project commit proceeds");
    run_holder.execute_batch("ROLLBACK").expect("release");
}
