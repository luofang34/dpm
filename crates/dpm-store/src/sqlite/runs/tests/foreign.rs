//! Runs recorded before a restore stay historical: nothing new is written to them, and runs under
//! the restored lineage work beside them.

use super::*;
use crate::{SqliteStore, restore_store_blocking};
use dpm_model::{LifecycleEntry, RunSnapshot};

/// A restored pair holding one run recorded before the restore, with its start and one waiting
/// transition and one activity record.
struct Restored {
    runs: RunStore,
    plan: Plan,
    work: WorkItemId,
    old: RunSnapshot,
    waiting: RunTransition,
    old_lineage: LineageId,
    new_lineage: LineageId,
    _directory: tempfile::TempDir,
}

fn restored() -> Restored {
    let directory = tempfile::tempdir().expect("directory");
    let (plan, work) = claimed();
    let path = directory.path().join("project.sqlite");
    let mut project = SqliteStore::open_blocking(&path).expect("project");
    project.initialize_blocking(&plan).expect("initialize");
    let old_lineage = project
        .lineage_blocking()
        .expect("lineage")
        .expect("lineage")
        .lineage_id;
    let mut runs = RunStore::open_or_create_blocking(
        &RunStore::sidecar_path(&path),
        plan.workspace.id,
        old_lineage,
    )
    .expect("runs");
    let record = record(&plan, old_lineage, &request(work));
    runs.start_blocking(&record, old_lineage).expect("start");
    let waiting = RunTransition {
        id: RunEventId::new(),
        run: record.id,
        to: RunState::Waiting,
        detail: None,
        observed_at: None,
    };
    runs.transition_blocking(&waiting, &worker(), Utc::now(), old_lineage)
        .expect("waiting");
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
        old_lineage,
    )
    .expect("activity");
    let backup = directory.path().join("backup.sqlite");
    project.backup_blocking(&backup).expect("backup");
    let target = directory.path().join("restored.sqlite");
    let report = restore_store_blocking(&backup, &target).expect("restore");
    assert_ne!(report.lineage_id, old_lineage);
    let runs =
        RunStore::open_existing_blocking(&RunStore::sidecar_path(&target), plan.workspace.id)
            .expect("open")
            .expect("exists");
    let old = runs
        .snapshot_blocking(record.id)
        .expect("read")
        .expect("run");
    Restored {
        runs,
        plan,
        work,
        old,
        waiting,
        old_lineage,
        new_lineage: report.lineage_id,
        _directory: directory,
    }
}

fn pages(runs: &RunStore) -> (dpm_model::LifecyclePage, dpm_model::ActivityPage) {
    (
        runs.lifecycle_page_blocking(0, 1000, None)
            .expect("lifecycle"),
        runs.activity_page_blocking(0, 1000, None)
            .expect("activity"),
    )
}

fn assert_foreign(
    result: Result<impl std::fmt::Debug, RunStoreError>,
    what: &str,
    restored: &Restored,
) {
    match result {
        Err(RunStoreError::ForeignRun {
            run,
            recorded,
            current,
        }) => {
            assert_eq!(run, restored.old.record.id, "{what}");
            assert_eq!(
                (recorded, current),
                (restored.old_lineage, restored.new_lineage),
                "{what}"
            );
        }
        other => panic!("{what}: expected ForeignRun, got {other:?}"),
    }
}

#[test]
fn new_lifecycle_activity_and_links_are_refused_for_a_run_from_before_the_restore() {
    let mut restored = restored();
    let before = pages(&restored.runs);
    let run = restored.old.record.id;
    let lineage = restored.new_lineage;
    let completed = RunTransition {
        id: RunEventId::new(),
        run,
        to: RunState::Completed,
        detail: None,
        observed_at: None,
    };
    assert_foreign(
        restored
            .runs
            .transition_blocking(&completed, &worker(), Utc::now(), lineage),
        "transition",
        &restored,
    );
    let activity = ActivityInput {
        run,
        source_sequence: 2,
        kind: ActivityKind::Heartbeat,
        text: None,
        observed_at: None,
    };
    assert_foreign(
        restored
            .runs
            .append_activity_blocking(&[activity], &worker(), Utc::now(), lineage),
        "activity",
        &restored,
    );
    let operation = OperationFacts {
        id: OperationId::new(),
        actor: worker(),
        timestamp: Utc::now(),
        work: Some(restored.work),
        workspace: restored.plan.workspace.id,
        lineage: restored.old_lineage,
    };
    assert_foreign(
        restored
            .runs
            .link_blocking(run, &operation, &worker(), Utc::now(), lineage),
        "link",
        &restored,
    );
    // An operation under the new lineage does not make the old run current either.
    let operation = OperationFacts {
        lineage,
        ..operation
    };
    assert_foreign(
        restored
            .runs
            .link_blocking(run, &operation, &worker(), Utc::now(), lineage),
        "link under the new lineage",
        &restored,
    );
    assert_eq!(pages(&restored.runs), before, "nothing was written");
    let after = restored
        .runs
        .snapshot_blocking(run)
        .expect("read")
        .expect("run");
    assert_eq!(
        after, restored.old,
        "the old run is exactly as it was restored"
    );
    assert_eq!(
        after.record.contract.lineage_id, restored.old_lineage,
        "attribution is not rewritten"
    );
}

#[test]
fn retries_for_a_run_from_before_the_restore_are_refused_not_replayed() {
    let mut restored = restored();
    let run = restored.old.record.id;
    let lineage = restored.new_lineage;
    let before = pages(&restored.runs);
    // A transition recorded before the restore, resent: refused, so a retry cannot bypass the
    // lineage boundary by hitting a recorded identity.
    assert_foreign(
        restored
            .runs
            .transition_blocking(&restored.waiting, &worker(), Utc::now(), lineage),
        "transition retry",
        &restored,
    );
    // So is a resent start, though its request matches the recorded one.
    let resent = restored.old.record.clone();
    assert_foreign(
        restored.runs.start_blocking(&resent, lineage),
        "start retry",
        &restored,
    );
    // And a resent activity record that is still retained.
    let activity = ActivityInput {
        run,
        source_sequence: 1,
        kind: ActivityKind::Heartbeat,
        text: None,
        observed_at: None,
    };
    assert_foreign(
        restored
            .runs
            .append_activity_blocking(&[activity], &worker(), Utc::now(), lineage),
        "activity retry",
        &restored,
    );
    assert_eq!(pages(&restored.runs), before);
}

#[test]
fn a_retry_still_meets_the_archive_and_lineage_checks_first() {
    let mut restored = restored();
    let run = restored.old.record.id;
    // The wrong lineage for the store is refused before the run is even looked at.
    let other = LineageId::new();
    assert!(matches!(
        restored
            .runs
            .transition_blocking(&restored.waiting, &worker(), Utc::now(), other),
        Err(RunStoreError::Store(StoreError::Lineage(
            LineageError::Mismatch { .. }
        )))
    ));
    restored
        .runs
        .connection
        .execute("UPDATE run_binding SET archived = 1", [])
        .expect("archive");
    assert!(matches!(
        restored.runs.transition_blocking(
            &restored.waiting,
            &worker(),
            Utc::now(),
            restored.new_lineage
        ),
        Err(RunStoreError::Store(StoreError::Lineage(
            LineageError::Archived { .. }
        )))
    ));
    let _ = run;
}

#[test]
fn runs_under_the_restored_lineage_work_beside_the_foreign_one() {
    let mut restored = restored();
    let lineage = restored.new_lineage;
    let fresh = record(&restored.plan, lineage, &request(restored.work));
    let started = restored
        .runs
        .start_blocking(&fresh, lineage)
        .expect("new run");
    assert!(!started.replayed);
    let completed = RunTransition {
        id: RunEventId::new(),
        run: fresh.id,
        to: RunState::Waiting,
        detail: None,
        observed_at: None,
    };
    let entry: LifecycleEntry = restored
        .runs
        .transition_blocking(&completed, &worker(), Utc::now(), lineage)
        .expect("lifecycle")
        .value;
    assert_eq!(entry.event.state, RunState::Waiting);
    restored
        .runs
        .append_activity_blocking(
            &[ActivityInput {
                run: fresh.id,
                source_sequence: 1,
                kind: ActivityKind::Heartbeat,
                text: None,
                observed_at: None,
            }],
            &worker(),
            Utc::now(),
            lineage,
        )
        .expect("activity");
    let operation = OperationFacts {
        id: OperationId::new(),
        actor: worker(),
        timestamp: fresh.started_at + TimeDelta::seconds(1),
        work: Some(restored.work),
        workspace: restored.plan.workspace.id,
        lineage,
    };
    restored
        .runs
        .link_blocking(fresh.id, &operation, &worker(), Utc::now(), lineage)
        .expect("link");
    // An exact retry of the new run's start is still answered as recorded.
    assert!(
        restored
            .runs
            .start_blocking(&fresh, lineage)
            .expect("retry")
            .replayed
    );
    let old = restored
        .runs
        .snapshot_blocking(restored.old.record.id)
        .expect("read")
        .expect("run");
    assert_eq!(old, restored.old);
}
