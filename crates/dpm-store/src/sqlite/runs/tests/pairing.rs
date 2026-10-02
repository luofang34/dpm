//! A project store and its run store must be a consistent pair, whichever copies they are.

use super::*;
use crate::{SqliteStore, restore_store_blocking, verify_store_blocking};
use std::{fs, path::Path};

/// A live project store with its run store and one run, in a temporary directory.
struct Pair {
    directory: tempfile::TempDir,
    project: SqliteStore,
    runs: RunStore,
    plan: Plan,
    work: WorkItemId,
    lineage: LineageId,
    run: RunRecord,
}

fn pair() -> Pair {
    let directory = tempfile::tempdir().expect("directory");
    let (plan, work) = claimed();
    let path = directory.path().join("project.sqlite");
    let mut project = SqliteStore::open_blocking(&path).expect("project");
    project.initialize_blocking(&plan).expect("initialize");
    let lineage = project
        .lineage_blocking()
        .expect("lineage")
        .expect("lineage")
        .lineage_id;
    let mut runs = RunStore::open_or_create_blocking(
        &RunStore::sidecar_path(&path),
        plan.workspace.id,
        lineage,
    )
    .expect("runs");
    let run = record(&plan, lineage, &request(work));
    runs.start_blocking(&run, lineage).expect("start");
    Pair {
        directory,
        project,
        runs,
        plan,
        work,
        lineage,
        run,
    }
}

impl Pair {
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.directory.path().join(name)
    }

    /// Commit a project Start of the worker and return its facts.
    fn project_start(&mut self) -> OperationFacts {
        let operation = apply_command(
            &mut self.plan,
            worker(),
            Command::Start {
                work: self.work,
                occurred_at: None,
            },
            Utc::now(),
            OperationId::new(),
        )
        .expect("start");
        self.project
            .persist_blocking(&self.plan, &operation, Some(self.lineage))
            .expect("persist");
        OperationFacts::of(&operation, self.plan.workspace.id, self.lineage)
    }

    fn link(&mut self, facts: &OperationFacts) {
        self.runs
            .link_blocking(self.run.id, facts, &worker(), Utc::now(), self.lineage)
            .expect("link");
    }

    fn backup(&self, name: &str) -> std::path::PathBuf {
        let to = self.path(name);
        self.project.backup_blocking(&to).expect("backup");
        to
    }
}

fn copy_runs(from: &Path, to: &Path) {
    fs::copy(RunStore::sidecar_path(from), RunStore::sidecar_path(to)).expect("copy run store");
}

fn refusal(result: Result<crate::IntegrityReport, StoreError>) -> StoreError {
    let error = result.expect_err("an inconsistent pair is refused");
    assert!(error.is_corruption(), "{error}");
    error
}

#[test]
fn a_run_store_copied_after_the_project_store_attributes_an_operation_the_project_copy_lacks() {
    let mut pair = pair();
    let earlier = pair.backup("earlier.sqlite");
    let facts = pair.project_start();
    pair.link(&facts);
    let later = pair.backup("later.sqlite");
    let report = verify_store_blocking(&later).expect("a consistent backup");
    assert_eq!(report.runs.expect("runs").runs, 1);
    // The earlier project copy paired with the later run store models copying the run store
    // second while an operation and its link arrive between the two copies.
    let swapped = pair.path("swapped.sqlite");
    fs::copy(&earlier, &swapped).expect("copy project");
    copy_runs(&later, &swapped);
    let error = refusal(verify_store_blocking(&swapped));
    assert!(error.to_string().contains("does not hold"), "{error}");
    // The other order is consistent: a later project copy holds everything an earlier run copy
    // names, and only operations that were never linked differ. This is why a backup copies the
    // run store first.
    let mixed = pair.path("mixed.sqlite");
    fs::copy(&later, &mixed).expect("copy project");
    copy_runs(&earlier, &mixed);
    let report = verify_store_blocking(&mixed).expect("a later project with an earlier run store");
    assert_eq!(report.operation_count, 1);
}

#[test]
fn a_backup_that_copied_the_run_store_first_is_a_consistent_pair_with_links() {
    let mut pair = pair();
    let facts = pair.project_start();
    pair.link(&facts);
    let backup = pair.backup("backup.sqlite");
    let report = verify_store_blocking(&backup).expect("pair");
    let runs = report.runs.expect("runs");
    assert!(runs.archived && report.archived);
    assert_eq!(runs.lineage_id, report.lineage_id);
    // Restoring it verifies the same pair again and forks both to one new lineage.
    let restored = restore_store_blocking(&backup, &pair.path("restored.sqlite")).expect("restore");
    assert_eq!(restored.runs.expect("runs").lineage_id, restored.lineage_id);
}

#[test]
fn a_restore_refuses_a_source_pair_that_is_not_consistent_and_creates_nothing() {
    let mut pair = pair();
    let earlier = pair.backup("earlier.sqlite");
    let facts = pair.project_start();
    pair.link(&facts);
    let later = pair.backup("later.sqlite");
    copy_runs(&later, &earlier);
    let target = pair.path("restored.sqlite");
    assert!(restore_store_blocking(&earlier, &target).is_err());
    assert!(!target.exists() && !RunStore::sidecar_path(&target).exists());
}

#[test]
fn a_run_store_bound_to_another_lineage_or_workspace_is_not_a_pair() {
    let pair = pair();
    let backup = pair.backup("backup.sqlite");
    let runs = RunStore::sidecar_path(&backup);
    let raw = rusqlite::Connection::open(&runs).expect("raw");
    raw.execute(
        "UPDATE run_binding SET lineage_id = ?1",
        [LineageId::new().to_string()],
    )
    .expect("rebind lineage");
    drop(raw);
    let error = refusal(verify_store_blocking(&backup));
    assert!(error.to_string().contains("bound to lineage"), "{error}");
    let raw = rusqlite::Connection::open(&runs).expect("raw");
    raw.execute(
        "UPDATE run_binding SET lineage_id = ?1, workspace_id = ?2",
        [
            pair.lineage.to_string(),
            dpm_model::WorkspaceId::new().to_string(),
        ],
    )
    .expect("rebind workspace");
    drop(raw);
    refusal(verify_store_blocking(&backup));
}

#[test]
fn a_run_that_observed_a_revision_the_project_history_lacks_is_not_a_pair() {
    let mut pair = pair();
    let earlier = pair.backup("earlier.sqlite");
    pair.project_start();
    // A second run starts after the project moved on, so it observed the later revision.
    let second = record(&pair.plan, pair.lineage, &request(pair.work));
    pair.runs
        .start_blocking(&second, pair.lineage)
        .expect("second run");
    let later = pair.backup("later.sqlite");
    let swapped = pair.path("swapped.sqlite");
    fs::copy(&earlier, &swapped).expect("copy project");
    copy_runs(&later, &swapped);
    let error = refusal(verify_store_blocking(&swapped));
    assert!(error.to_string().contains("does not hold"), "{error}");
}

#[test]
fn a_link_that_no_longer_passes_the_link_rules_is_not_a_pair() {
    let mut pair = pair();
    let facts = pair.project_start();
    pair.link(&facts);
    // End the run, then commit a later operation and re-point the link at it: the operation
    // exists in the project history but happened after the run ended.
    pair.runs
        .transition_blocking(
            &RunTransition {
                id: RunEventId::new(),
                run: pair.run.id,
                to: RunState::Completed,
                detail: None,
                observed_at: None,
            },
            &worker(),
            Utc::now(),
            pair.lineage,
        )
        .expect("complete");
    let later = apply_command(
        &mut pair.plan,
        worker(),
        Command::Block {
            work: pair.work,
            reason: "after the run".into(),
        },
        Utc::now() + TimeDelta::seconds(5),
        OperationId::new(),
    )
    .expect("block");
    pair.project
        .persist_blocking(&pair.plan, &later, Some(pair.lineage))
        .expect("persist");
    let backup = pair.backup("backup.sqlite");
    verify_store_blocking(&backup).expect("consistent before the tamper");
    let raw = rusqlite::Connection::open(RunStore::sidecar_path(&backup)).expect("raw");
    raw.execute(
        "UPDATE run_links SET operation_id = ?1",
        [later.id.to_string()],
    )
    .expect("re-point");
    drop(raw);
    let error = refusal(verify_store_blocking(&backup));
    assert!(error.to_string().contains("fails its rules"), "{error}");
}

#[test]
fn a_run_store_that_was_never_paired_leaves_a_store_without_runs_unaffected() {
    let pair = pair();
    let plain = tempfile::tempdir().expect("directory");
    let path = plain.path().join("plain.sqlite");
    let mut store = SqliteStore::open_blocking(&path).expect("store");
    store.initialize_blocking(&pair.plan).expect("initialize");
    let report = verify_store_blocking(&path).expect("verify");
    assert!(report.runs.is_none());
}

/// Revisions wrap at `u64::MAX`, so a run's observed basis is checked as membership in the
/// history, never as a number below the snapshot's.
#[test]
fn a_run_observed_before_the_revision_counter_wrapped_is_a_consistent_pair() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut plan, work) = unclaimed();
    plan.revision = u64::MAX - 1;
    let path = directory.path().join("project.sqlite");
    let mut project = SqliteStore::open_blocking(&path).expect("project");
    project
        .initialize_blocking(&plan)
        .expect("a plan at the edge of the counter");
    let lineage = project
        .lineage_blocking()
        .expect("lineage")
        .expect("lineage")
        .lineage_id;
    let mut commit = |plan: &mut Plan, command| {
        let operation = apply_command(plan, worker(), command, Utc::now(), OperationId::new())
            .expect("command");
        project
            .persist_blocking(plan, &operation, Some(lineage))
            .expect("persist");
        operation
    };
    commit(&mut plan, Command::Claim { work });
    assert_eq!(plan.revision, u64::MAX);
    let mut runs = RunStore::open_or_create_blocking(
        &RunStore::sidecar_path(&path),
        plan.workspace.id,
        lineage,
    )
    .expect("runs");
    let run = record(&plan, lineage, &request(work));
    assert_eq!(
        run.contract.revision,
        u64::MAX,
        "observed at the last value"
    );
    runs.start_blocking(&run, lineage).expect("start");
    let started = commit(
        &mut plan,
        Command::Start {
            work,
            occurred_at: None,
        },
    );
    assert_eq!(plan.revision, 0, "the counter wrapped");
    let facts = OperationFacts::of(&started, plan.workspace.id, lineage);
    runs.link_blocking(run.id, &facts, &worker(), Utc::now(), lineage)
        .expect("link");
    drop(runs);
    let backup = directory.path().join("backup.sqlite");
    let report = project
        .backup_blocking(&backup)
        .expect("backup across the wrap");
    assert_eq!(report.runs.expect("runs").runs, 1);
    verify_store_blocking(&path).expect("the live pair");
    let restored = directory.path().join("restored.sqlite");
    let report = restore_store_blocking(&backup, &restored).expect("restore across the wrap");
    assert_eq!(report.revision, 0);
    // A run claiming a revision no history holds is still refused, wrap or not.
    let raw = rusqlite::Connection::open(RunStore::sidecar_path(&backup)).expect("raw");
    let forged = serde_json::to_string(&{
        let mut forged = run.clone();
        forged.contract.revision = 12345;
        forged
    })
    .expect("json");
    raw.execute("UPDATE runs SET record_json = ?1", [forged])
        .expect("forge");
    drop(raw);
    refusal(verify_store_blocking(&backup));
}
