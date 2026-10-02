//! A backup archive is never a live workspace: no run write opens, creates or replays anything.

use super::*;

fn heartbeat(run: RunId, source_sequence: u64) -> RunActivityRequest {
    activity(run, source_sequence, ActivityKind::Heartbeat)
}

/// A project with a claimed and started task, backed up, and the archive opened as an application.
fn archive_with(directory: &std::path::Path, runs: bool) -> (Application, RunStartRequest) {
    let mut app =
        Application::initialize_blocking(&directory.join("state.sqlite"), &fixture_plan())
            .expect("app");
    let task = work(&app);
    project(&mut app, &worker(), Command::Claim { work: task });
    project(
        &mut app,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    let request = start_request();
    if runs {
        app.start_run_blocking(request.clone()).expect("run");
        let run = request.run_id.expect("id");
        app.record_run_activity_blocking(heartbeat(run, 1))
            .expect("activity");
    }
    let archive = directory.join("archive.sqlite");
    app.backup_blocking(&archive).expect("backup");
    (
        Application::open_blocking(&archive).expect("open the archive"),
        request,
    )
}

fn assert_archived(error: AppError, what: &str) {
    assert_eq!(error.code(), "archived_store", "{what}: {error}");
}

fn first_operation(app: &Application) -> dpm_model::OperationId {
    app.history_blocking(0, 10).expect("history").entries[0]
        .operation
        .operation
        .id
}

#[test]
fn a_run_free_backup_archive_takes_no_run_writes_and_creates_no_run_store() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut archive, request) = archive_with(directory.path(), false);
    let sidecar = directory.path().join("archive.sqlite.runs");
    assert!(!sidecar.exists());
    assert_archived(
        archive.start_run_blocking(request).expect_err("start"),
        "start",
    );
    let run = RunId::new();
    assert_archived(
        archive
            .report_run_blocking(report(run, RunState::Failed))
            .expect_err("report"),
        "report",
    );
    assert_archived(
        archive
            .record_run_activity_blocking(heartbeat(run, 1))
            .expect_err("activity"),
        "activity",
    );
    let operation = first_operation(&archive);
    assert_archived(
        archive
            .link_run_operation_blocking(RunLinkRequest {
                actor: worker(),
                run,
                operation,
                base_lineage: None,
            })
            .expect_err("link"),
        "link",
    );
    assert!(!sidecar.exists(), "a refused write created nothing");
    // Reads stay available and answer empty.
    let runs = archive
        .runs_blocking(&RunQuery {
            key: None,
            limit: 10,
        })
        .expect("runs");
    assert!(runs.data.runs.is_empty());
    let feed = archive.run_lifecycle_blocking(0, 10, None).expect("feed");
    assert_eq!(feed.data.head_sequence, 0);
    assert!(!sidecar.exists(), "reads created nothing either");
}

#[test]
fn an_archive_with_runs_refuses_every_write_including_exact_retries_and_stays_readable() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut archive, request) = archive_with(directory.path(), true);
    let run = request.run_id.expect("id");
    let sidecar = directory.path().join("archive.sqlite.runs");
    let before = std::fs::read(&sidecar).expect("run archive");
    let lifecycle = archive
        .run_lifecycle_blocking(0, 100, None)
        .expect("feed")
        .data;
    let feed = archive
        .run_activity_blocking(0, 100, None)
        .expect("feed")
        .data;
    // An exact retry of the start is refused rather than answered from the record.
    assert_archived(
        archive
            .start_run_blocking(request)
            .expect_err("start retry"),
        "start retry",
    );
    assert_archived(
        archive
            .report_run_blocking(report(run, RunState::Completed))
            .expect_err("report"),
        "report",
    );
    // So is an exact resend of a retained activity record.
    assert_archived(
        archive
            .record_run_activity_blocking(heartbeat(run, 1))
            .expect_err("activity retry"),
        "activity retry",
    );
    let operation = first_operation(&archive);
    assert_archived(
        archive
            .link_run_operation_blocking(RunLinkRequest {
                actor: worker(),
                run,
                operation,
                base_lineage: None,
            })
            .expect_err("link"),
        "link",
    );
    assert_eq!(
        std::fs::read(&sidecar).expect("run archive"),
        before,
        "the run archive is untouched"
    );
    assert_eq!(
        archive
            .run_lifecycle_blocking(0, 100, None)
            .expect("feed")
            .data,
        lifecycle
    );
    assert_eq!(
        archive
            .run_activity_blocking(0, 100, None)
            .expect("feed")
            .data,
        feed
    );
    let view = archive.run_blocking(run).expect("a read is allowed").data;
    assert_eq!(view.state, RunState::Working);
}

/// An archived run store beside a live project of the same workspace and lineage is not
/// writable: the store's own transaction refuses, an exact start retry included.
#[test]
fn an_archived_run_store_beside_a_live_project_refuses_every_write_including_start_retries() {
    let directory = tempfile::tempdir().expect("directory");
    let live = directory.path().join("state.sqlite");
    let mut app = Application::initialize_blocking(&live, &fixture_plan()).expect("app");
    let task = work(&app);
    project(&mut app, &worker(), Command::Claim { work: task });
    let request = start_request();
    app.start_run_blocking(request.clone()).expect("run");
    let run = request.run_id.expect("id");
    app.record_run_activity_blocking(heartbeat(run, 1))
        .expect("activity");
    let archive = directory.path().join("archive.sqlite");
    app.backup_blocking(&archive).expect("backup");
    drop(app);
    // The live project keeps its own lineage, but its run store is now the archived copy.
    let sidecar = directory.path().join("state.sqlite.runs");
    std::fs::copy(directory.path().join("archive.sqlite.runs"), &sidecar).expect("swap");
    let before = std::fs::read(&sidecar).expect("run store");
    let mut app = Application::open_blocking(&live).expect("open");
    assert_archived(
        app.start_run_blocking(request).expect_err("start retry"),
        "start retry",
    );
    assert_archived(
        app.report_run_blocking(report(run, RunState::Completed))
            .expect_err("report"),
        "report",
    );
    assert_archived(
        app.record_run_activity_blocking(heartbeat(run, 2))
            .expect_err("activity"),
        "activity",
    );
    let mut fresh = start_request();
    fresh.run_id = Some(RunId::new());
    assert_archived(
        app.start_run_blocking(fresh).expect_err("new run"),
        "new run",
    );
    assert_eq!(std::fs::read(&sidecar).expect("run store"), before);
    assert_eq!(
        app.run_blocking(run).expect("reads are allowed").data.state,
        RunState::Working
    );
}
