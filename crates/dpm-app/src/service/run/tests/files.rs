//! Runs against real files: restart, orphaned recovery, backup and restore.

use super::*;
use dpm_model::{LineageStatus, ObservedStatus};
use std::path::Path;

fn project(directory: &Path) -> Application {
    let mut app =
        Application::initialize_blocking(&directory.join("state.sqlite"), &fixture_plan())
            .expect("app");
    let id = work(&app);
    super::project(&mut app, &worker(), Command::Claim { work: id });
    super::project(
        &mut app,
        &worker(),
        Command::Start {
            work: id,
            occurred_at: None,
        },
    );
    app
}

fn names(directory: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(directory)
        .expect("directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn reading_runs_never_creates_the_run_store_and_the_first_write_does() {
    let directory = tempfile::tempdir().expect("directory");
    let mut app = project(directory.path());
    let before = names(directory.path());
    assert!(
        app.runs_blocking(&RunQuery {
            key: None,
            limit: 10
        })
        .expect("runs")
        .data
        .runs
        .is_empty()
    );
    assert_eq!(
        app.run_lifecycle_blocking(0, 10, None)
            .expect("feed")
            .data
            .head_sequence,
        0
    );
    assert_eq!(
        app.run_activity_blocking(0, 10, None)
            .expect("feed")
            .data
            .head_sequence,
        0
    );
    assert!(app.run_blocking(RunId::new()).is_err());
    assert_eq!(names(directory.path()), before, "reads created nothing");
    assert!(!directory.path().join("state.sqlite.runs").exists());
    start(&mut app);
    assert!(directory.path().join("state.sqlite.runs").exists());
}

#[test]
fn a_restart_recovers_known_runs_and_shows_lost_telemetry_as_stale_not_finished() {
    let directory = tempfile::tempdir().expect("directory");
    let mut app = project(directory.path());
    let run = start(&mut app);
    app.record_run_activity_blocking(activity(run, 1, ActivityKind::ToolStarted))
        .expect("activity");
    let known = app.run_blocking(run).expect("run").data;
    drop(app);
    let mut app =
        Application::open_blocking(directory.path().join("state.sqlite")).expect("reopen");
    let recovered = app.run_blocking(run).expect("recovered").data;
    assert_eq!(recovered.run, known.run);
    assert_eq!(recovered.activity, known.activity);
    assert_eq!(recovered.state, RunState::Working, "no one reported an end");
    // The process that was running died with the old application: nothing more arrives.
    at(&mut app, known.last_receipt_at + TimeDelta::hours(1));
    let later = app.run_blocking(run).expect("run").data;
    assert_eq!(later.status, ObservedStatus::Stale);
    assert_eq!(later.state, RunState::Working);
    assert_eq!(later.lineage, LineageStatus::Current);
    // New receipts after the restart are accepted and refresh it.
    let receipt = app
        .record_run_activity_blocking(activity(run, 2, ActivityKind::Heartbeat))
        .expect("activity");
    let received = receipt.data.entries[0].entry.record.recorded_at;
    at(&mut app, received + TimeDelta::seconds(5));
    assert_eq!(
        app.run_blocking(run).expect("run").data.status,
        ObservedStatus::Working
    );
}

#[test]
fn a_project_backup_carries_runs_and_a_restore_labels_them_foreign_without_reattaching() {
    let directory = tempfile::tempdir().expect("directory");
    let mut app = project(directory.path());
    let run = start(&mut app);
    app.record_run_activity_blocking(activity(run, 1, ActivityKind::Heartbeat))
        .expect("activity");
    let original = app.lineage_blocking().expect("lineage").expect("lineage");
    let backup = directory.path().join("backup.sqlite");
    let report = app.backup_blocking(&backup).expect("backup");
    let carried = report
        .runs
        .expect("the backup reports the run store it carried");
    assert_eq!((carried.runs, carried.activity_retained), (1, 1));
    let restored_path = directory.path().join("restored.sqlite");
    let restored = crate::restore_store_blocking(&backup, &restored_path).expect("restore");
    assert_ne!(restored.lineage_id, original);
    assert_eq!(restored.runs.expect("runs restored").runs, 1);
    let mut app = Application::open_blocking(&restored_path).expect("open");
    let view = app.run_blocking(run).expect("run").data;
    assert_eq!(
        view.lineage,
        LineageStatus::Foreign,
        "recorded under the history before the restore"
    );
    assert_eq!(
        view.status,
        ObservedStatus::Unknown,
        "a process of another history is not known to run"
    );
    assert_eq!(
        view.run.contract.lineage_id, original,
        "attribution is not rewritten"
    );
    assert_eq!(view.state, RunState::Working);
    assert!(view.orphan.is_none());
    // New runs record under the restored lineage and sit beside the foreign one.
    let fresh = start(&mut app);
    let views = app
        .runs_blocking(&RunQuery {
            key: None,
            limit: 10,
        })
        .expect("runs")
        .data
        .runs;
    assert_eq!(views.len(), 2);
    let current = views
        .iter()
        .find(|view| view.run.id == fresh)
        .expect("fresh");
    assert_eq!(current.lineage, LineageStatus::Current);
    assert_eq!(current.run.contract.lineage_id, restored.lineage_id);
    // The live store still verifies with its runs.
    assert_eq!(
        crate::verify_store_blocking(&directory.path().join("state.sqlite"))
            .expect("verify")
            .runs
            .expect("runs")
            .runs,
        1
    );
}

#[test]
fn a_run_store_of_another_workspace_is_refused_without_changing_either_file() {
    let directory = tempfile::tempdir().expect("directory");
    let mut app = project(directory.path());
    start(&mut app);
    drop(app);
    let other = tempfile::tempdir().expect("directory");
    let mut plan = fixture_plan();
    plan.workspace.id = dpm_model::WorkspaceId::new();
    let foreign =
        Application::initialize_blocking(&other.path().join("state.sqlite"), &plan).expect("other");
    drop(foreign);
    // Move this workspace's run store beside the other project store.
    std::fs::copy(
        directory.path().join("state.sqlite.runs"),
        other.path().join("state.sqlite.runs"),
    )
    .expect("copy");
    let runs = other.path().join("state.sqlite.runs");
    let before = std::fs::read(&runs).expect("run store");
    let app = Application::open_blocking(other.path().join("state.sqlite")).expect("open project");
    let refused = app
        .runs_blocking(&RunQuery {
            key: None,
            limit: 10,
        })
        .expect_err("another workspace's runs");
    assert_eq!(refused.code(), "workspace_mismatch");
    assert_eq!(std::fs::read(&runs).expect("run store"), before);
    assert!(
        !names(other.path())
            .iter()
            .any(|name| name.starts_with("state.sqlite.runs-")),
        "no side file was created for the refused store"
    );
}

#[test]
fn a_damaged_run_store_is_refused_and_leaves_the_project_usable() {
    let directory = tempfile::tempdir().expect("directory");
    let mut app = project(directory.path());
    start(&mut app);
    drop(app);
    std::fs::write(
        directory.path().join("state.sqlite.runs"),
        b"this is not a database at all, just text",
    )
    .expect("damage");
    let mut app =
        Application::open_blocking(directory.path().join("state.sqlite")).expect("open project");
    let error = app
        .runs_blocking(&RunQuery {
            key: None,
            limit: 10,
        })
        .expect_err("damaged");
    assert!(
        ["corrupt_store", "storage_error"].contains(&error.code()),
        "{}",
        error.code()
    );
    assert_eq!(
        std::fs::read(directory.path().join("state.sqlite.runs")).expect("bytes"),
        b"this is not a database at all, just text",
        "the damaged file was not replaced"
    );
    assert!(app.start_run_blocking(start_request()).is_err());
    assert_eq!(
        app.status_blocking(false).expect("status").revision,
        2,
        "project queries are unaffected"
    );
}
