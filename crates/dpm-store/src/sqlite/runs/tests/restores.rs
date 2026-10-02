//! Runs and restores across generations: the fork a restore records is the evidence that keeps a
//! run's observed basis provable when its lineage has no operations of its own.

use super::*;
use crate::{SqliteStore, restore_store_blocking, verify_store_blocking};
use std::path::{Path, PathBuf};

/// A restored project store, its path and the lineage it continues.
fn restore_generation(
    directory: &Path,
    backup: &Path,
    name: &str,
) -> (SqliteStore, PathBuf, LineageId) {
    let path = directory.join(name);
    restore_store_blocking(backup, &path).expect("restore");
    let store = SqliteStore::open_existing_blocking(&path).expect("open");
    let lineage = store
        .lineage_blocking()
        .expect("lineage")
        .expect("lineage")
        .lineage_id;
    (store, path, lineage)
}

/// Start a run in `store`'s lineage at its current snapshot, creating the run store if needed.
fn run_in(store: &SqliteStore, path: &Path, work: WorkItemId) -> RunRecord {
    let plan = store.load_blocking().expect("load").expect("plan");
    let lineage = store
        .lineage_blocking()
        .expect("lineage")
        .expect("lineage")
        .lineage_id;
    let mut runs = RunStore::open_or_create_blocking(
        &RunStore::sidecar_path(path),
        plan.workspace.id,
        lineage,
    )
    .expect("runs");
    let run = record(&plan, lineage, &request(work));
    runs.start_blocking(&run, lineage).expect("start");
    run
}

/// A project with two committed operations (Claim and Start, revision 2 under its lineage), made
/// from a plan that starts at revision 0, so that no run observed the genesis revision.
fn operated(directory: &Path, name: &str) -> (SqliteStore, PathBuf, WorkItemId) {
    let (mut plan, work) = unclaimed();
    let path = directory.join(name);
    let mut store = SqliteStore::open_blocking(&path).expect("project");
    store.initialize_blocking(&plan).expect("initialize");
    let lineage = store
        .lineage_blocking()
        .expect("lineage")
        .expect("lineage")
        .lineage_id;
    for command in [
        Command::Claim { work },
        Command::Start {
            work,
            occurred_at: None,
        },
    ] {
        let operation = apply_command(&mut plan, worker(), command, Utc::now(), OperationId::new())
            .expect("command");
        store
            .persist_blocking(&plan, &operation, Some(lineage))
            .expect("persist");
    }
    assert_eq!(plan.revision, 2);
    (store, path, work)
}

/// A run started in a restored lineage before that lineage has any semantic operation observes a
/// revision no project operation names under that lineage. The fork the restore recorded is the
/// evidence, and it keeps verifying through repeated restores, whether or not the first
/// generation had a run store.
#[test]
fn runs_started_in_restored_lineages_before_any_operation_survive_repeated_restores() {
    for first_has_runs in [false, true] {
        let directory = tempfile::tempdir().expect("directory");
        let (a, a_path, work) = operated(directory.path(), "a.sqlite");
        if first_has_runs {
            run_in(&a, &a_path, work);
        }
        let a_backup = directory.path().join("a-backup.sqlite");
        a.backup_blocking(&a_backup).expect("backup of A");

        let (b, b_path, b_lineage) = restore_generation(directory.path(), &a_backup, "b.sqlite");
        let b_run = run_in(&b, &b_path, work);
        assert_eq!(b_run.contract.revision, 2);
        let b_backup = directory.path().join("b-backup.sqlite");
        b.backup_blocking(&b_backup)
            .expect("backup of B holding a run observed at its start");
        verify_store_blocking(&b_path).expect("the live pair");

        let (c, c_path, c_lineage) = restore_generation(directory.path(), &b_backup, "c.sqlite");
        assert_ne!(c_lineage, b_lineage);
        let c_run = run_in(&c, &c_path, work);
        verify_store_blocking(&c_path).expect("a restore of a restore");
        let c_backup = directory.path().join("c-backup.sqlite");
        c.backup_blocking(&c_backup).expect("backup of C");

        let (_, d_path, _) = restore_generation(directory.path(), &c_backup, "d.sqlite");
        let report = verify_store_blocking(&d_path).expect("three restores deep");
        // Every earlier generation's run is still there, attributed to the lineage it was
        // recorded under, and none was reattached.
        let runs =
            RunStore::open_existing_blocking(&RunStore::sidecar_path(&d_path), report.workspace_id)
                .expect("open")
                .expect("exists");
        let held = runs.snapshots_blocking(None, 10).expect("runs");
        let lineages: Vec<_> = held
            .iter()
            .map(|run| run.record.contract.lineage_id)
            .collect();
        assert!(lineages.contains(&b_lineage) && lineages.contains(&c_lineage));
        let ids: Vec<_> = held.iter().map(|run| run.record.id).collect();
        assert!(ids.contains(&b_run.id) && ids.contains(&c_run.id));
    }
}

#[test]
fn a_run_started_before_its_lineages_first_operation_verifies_after_that_operation_and_a_restore() {
    let directory = tempfile::tempdir().expect("directory");
    let (a, _, work) = operated(directory.path(), "a.sqlite");
    let a_backup = directory.path().join("a-backup.sqlite");
    a.backup_blocking(&a_backup).expect("backup");
    let (mut b, b_path, b_lineage) = restore_generation(directory.path(), &a_backup, "b.sqlite");
    run_in(&b, &b_path, work);
    // B now commits its first semantic operation; the early run observed the revision it started
    // from, which that operation names as its base.
    let mut plan = b.load_blocking().expect("load").expect("plan");
    let operation = apply_command(
        &mut plan,
        worker(),
        Command::ReportProgress {
            work,
            percent: 10,
            note: None,
        },
        Utc::now(),
        OperationId::new(),
    )
    .expect("progress");
    b.persist_blocking(&plan, &operation, Some(b_lineage))
        .expect("persist");
    verify_store_blocking(&b_path).expect("live pair after the first operation");
    let b_backup = directory.path().join("b-backup.sqlite");
    b.backup_blocking(&b_backup).expect("backup");
    let (_, c_path, _) = restore_generation(directory.path(), &b_backup, "c.sqlite");
    let report = verify_store_blocking(&c_path).expect("restored");
    assert_eq!(report.runs.expect("runs").runs, 1);
}

#[test]
fn fork_evidence_cannot_legitimize_a_revision_the_history_does_not_hold() {
    let directory = tempfile::tempdir().expect("directory");
    let (a, _, work) = operated(directory.path(), "a.sqlite");
    let a_backup = directory.path().join("a-backup.sqlite");
    a.backup_blocking(&a_backup).expect("backup");
    let (b, b_path, b_lineage) = restore_generation(directory.path(), &a_backup, "b.sqlite");
    let early = run_in(&b, &b_path, work);
    let b_backup = directory.path().join("b-backup.sqlite");
    b.backup_blocking(&b_backup).expect("backup");
    verify_store_blocking(&b_backup).expect("consistent before the forgery");
    // Forge both the fork and the run to a revision the parent lineage never held: the fork must
    // itself be held by the lineage it names, so a pair of matching forgeries is still refused.
    let raw = rusqlite::Connection::open(RunStore::sidecar_path(&b_backup)).expect("raw");
    raw.execute(
        "UPDATE run_lineage_forks SET revision = 12345 WHERE lineage_id = ?1",
        [b_lineage.to_string()],
    )
    .expect("forge the fork");
    let mut forged = early.clone();
    forged.contract.revision = 12345;
    raw.execute(
        "UPDATE runs SET record_json = ?1",
        [serde_json::to_string(&forged).expect("json")],
    )
    .expect("forge the run");
    drop(raw);
    let error = verify_store_blocking(&b_backup).expect_err("forged evidence");
    assert!(error.is_corruption(), "{error}");
    assert!(error.to_string().contains("does not hold"), "{error}");
}
