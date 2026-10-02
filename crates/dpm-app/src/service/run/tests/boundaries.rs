//! The boundaries of retry, lineage and revision, through the application's own commands.

use super::*;
use dpm_model::{Artifact, ArtifactId, ArtifactKind, LineageStatus};
use std::collections::BTreeMap;

fn heartbeat(run: RunId, source_sequence: u64) -> RunActivityRequest {
    activity(run, source_sequence, ActivityKind::Heartbeat)
}

#[test]
fn a_pruned_retry_is_refused_as_expired_and_leaves_the_run_exactly_as_it_was() {
    let mut app = started();
    app.set_run_retention(1);
    let run = start(&mut app);
    app.record_run_activity_blocking(heartbeat(run, 1))
        .expect("first");
    app.record_run_activity_blocking(heartbeat(run, 2))
        .expect("second");
    let before = app.run_blocking(run).expect("run").data;
    let feed = app.run_activity_blocking(0, 100, None).expect("feed").data;
    let refused = app
        .record_run_activity_blocking(heartbeat(run, 1))
        .expect_err("a retry that outlived retention");
    assert_eq!(refused.code(), "activity_expired");
    let details = refused.details().expect("details");
    assert_eq!(details["high_water"], 2);
    assert_eq!(details["source_sequence"], 1);
    // Receipt time, tally and feed did not move, so derived staleness is unchanged.
    let after = app.run_blocking(run).expect("run").data;
    assert_eq!(after.last_receipt_at, before.last_receipt_at);
    assert_eq!(after.activity, before.activity);
    assert_eq!(
        app.run_activity_blocking(0, 100, None).expect("feed").data,
        feed
    );
    at(&mut app, before.last_receipt_at + TimeDelta::seconds(301));
    assert_eq!(
        app.run_blocking(run).expect("run").data.status,
        ObservedStatus::Stale
    );
}

#[test]
fn a_changed_tail_beyond_the_kept_text_is_a_conflict_through_the_application() {
    let mut app = started();
    let run = start(&mut app);
    let prefix = "x".repeat(dpm_model::MAX_ACTIVITY_TEXT_BYTES);
    let with = |tail: &str| {
        let mut request = activity(run, 1, ActivityKind::ToolResult);
        request.entries[0].text = Some(format!("{prefix}{tail}"));
        request
    };
    let first = app.record_run_activity_blocking(with("A")).expect("stored");
    assert!(first.data.entries[0].entry.record.truncated);
    assert!(
        app.record_run_activity_blocking(with("A"))
            .expect("exact resend")
            .data
            .entries[0]
            .duplicate
    );
    let refused = app
        .record_run_activity_blocking(with("B"))
        .expect_err("changed tail");
    assert_eq!(refused.code(), "duplicate_run_record");
}

/// A project and its run store, backed up and restored through the application.
fn restored_app(directory: &std::path::Path) -> (Application, RunId, dpm_model::LineageId) {
    let mut app =
        Application::initialize_blocking(&directory.join("state.sqlite"), &fixture_plan())
            .expect("app");
    let id = work(&app);
    project(&mut app, &worker(), Command::Claim { work: id });
    let run = start(&mut app);
    let original = app.lineage_blocking().expect("lineage").expect("lineage");
    let backup = directory.join("backup.sqlite");
    app.backup_blocking(&backup).expect("backup");
    let target = directory.join("restored.sqlite");
    let report = crate::restore_store_blocking(&backup, &target).expect("restore");
    assert_ne!(report.lineage_id, original);
    (
        Application::open_blocking(&target).expect("open"),
        run,
        original,
    )
}

#[test]
fn after_a_restore_a_run_from_before_it_takes_no_new_facts_and_new_runs_work() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, old, original) = restored_app(directory.path());
    let before = app.run_blocking(old).expect("run").data;
    assert_eq!(before.lineage, LineageStatus::Foreign);
    let lifecycle = app.run_lifecycle_blocking(0, 100, None).expect("feed").data;
    let activity_feed = app.run_activity_blocking(0, 100, None).expect("feed").data;
    // Every kind of write is refused with the stable code, and nothing is recorded.
    let foreign = |error: AppError| {
        assert_eq!(error.code(), "foreign_run", "{error}");
        let details = error.details().expect("details");
        assert_eq!(details["recorded"], serde_json::json!(original));
    };
    foreign(
        app.report_run_blocking(report(old, RunState::Completed))
            .expect_err("lifecycle"),
    );
    foreign(
        app.record_run_activity_blocking(heartbeat(old, 1))
            .expect_err("activity"),
    );
    let task = work(&app);
    let operation = project(
        &mut app,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    foreign(
        app.link_run_operation_blocking(RunLinkRequest {
            actor: worker(),
            run: old,
            operation: operation.operation.id,
            base_lineage: None,
        })
        .expect_err("link"),
    );
    // Resending the start of the old run is a retry across the boundary: refused, not answered.
    let mut resend = start_request();
    resend.run_id = Some(old);
    foreign(app.start_run_blocking(resend).expect_err("start retry"));
    assert_eq!(app.run_blocking(old).expect("run").data.run, before.run);
    assert_eq!(
        app.run_lifecycle_blocking(0, 100, None).expect("feed").data,
        lifecycle
    );
    assert_eq!(
        app.run_activity_blocking(0, 100, None).expect("feed").data,
        activity_feed
    );
    // A run under the restored lineage works completely beside it, linked operation included.
    let fresh = start(&mut app);
    app.record_run_activity_blocking(heartbeat(fresh, 1))
        .expect("activity");
    let progress = project(
        &mut app,
        &worker(),
        Command::ReportProgress {
            work: task,
            percent: 10,
            note: None,
        },
    );
    let linked = app
        .link_run_operation_blocking(RunLinkRequest {
            actor: worker(),
            run: fresh,
            operation: progress.operation.id,
            base_lineage: None,
        })
        .expect("link");
    assert_eq!(linked.data.run.lineage, LineageStatus::Current);
    app.report_run_blocking(report(fresh, RunState::Completed))
        .expect("complete");
    let old_after = app.run_blocking(old).expect("run").data;
    assert_eq!(
        (old_after.state, old_after.status),
        (RunState::Working, ObservedStatus::Unknown)
    );
}

/// Revisions wrap at `u64::MAX`; a run that observed the last value must still pair with a
/// project that has moved on past the wrap.
#[test]
fn a_run_observed_at_the_last_revision_survives_the_wrap_backup_verify_and_restore() {
    let directory = tempfile::tempdir().expect("directory");
    let mut plan = fixture_plan();
    plan.revision = u64::MAX - 1;
    let path = directory.path().join("state.sqlite");
    let mut app = Application::initialize_blocking(&path, &plan).expect("app at the edge");
    let id = work(&app);
    project(&mut app, &worker(), Command::Claim { work: id });
    assert_eq!(
        app.revision_blocking().expect("revision").revision,
        u64::MAX
    );
    let run = start(&mut app);
    let observed = app
        .run_blocking(run)
        .expect("run")
        .data
        .run
        .contract
        .revision;
    assert_eq!(observed, u64::MAX);
    let started = project(
        &mut app,
        &worker(),
        Command::Start {
            work: id,
            occurred_at: None,
        },
    );
    assert_eq!(
        started.operation.resulting_revision, 0,
        "the counter wrapped"
    );
    app.link_run_operation_blocking(RunLinkRequest {
        actor: worker(),
        run,
        operation: started.operation.id,
        base_lineage: None,
    })
    .expect("link across the wrap");
    let backup = directory.path().join("backup.sqlite");
    let report = app
        .backup_blocking(&backup)
        .expect("backup across the wrap");
    assert_eq!(report.runs.expect("runs").runs, 1);
    crate::verify_store_blocking(&path).expect("live verify");
    let restored = directory.path().join("restored.sqlite");
    let report = crate::restore_store_blocking(&backup, &restored).expect("restore");
    assert_eq!(report.revision, 0);
    let reopened = Application::open_blocking(&restored).expect("open");
    assert_eq!(
        reopened
            .run_blocking(run)
            .expect("run")
            .data
            .operations
            .len(),
        1
    );
}

fn attach(app: &mut Application, artifact: Artifact) -> ArtifactId {
    let id = artifact.id;
    project(
        app,
        &worker(),
        Command::AttachArtifact {
            work: work(app),
            artifact,
        },
    );
    id
}

#[test]
fn artifact_sources_must_be_exact_attached_evidence_and_are_captured_in_the_run() {
    let mut app = Application::in_memory_blocking(&fixture_plan()).expect("app");
    let id = work(&app);
    project(&mut app, &worker(), Command::Claim { work: id });
    let asset = *app
        .plan_blocking()
        .expect("plan")
        .assets
        .keys()
        .next()
        .expect("asset");
    let commit = "9".repeat(40);
    let evidence = |uri: String, metadata: BTreeMap<String, String>| Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::GitCommit,
        uri,
        label: "HEAD".into(),
        metadata,
        created_by: worker(),
        created_at: Utc::now(),
    };
    let branch = attach(&mut app, evidence("git:local@main".into(), BTreeMap::new()));
    let mut request = start_request();
    request.sources = vec![dpm_model::RunSource::Artifact { artifact: branch }];
    let refused = app
        .start_run_blocking(request)
        .expect_err("a branch is not exact");
    assert_eq!(refused.code(), "invalid_command");
    let exact = attach(
        &mut app,
        evidence(
            format!("git:asset:{asset}@{commit}"),
            BTreeMap::from([
                ("commit".to_string(), commit.clone()),
                ("asset_id".to_string(), asset.to_string()),
            ]),
        ),
    );
    let mut request = start_request();
    request.sources = vec![dpm_model::RunSource::Artifact { artifact: exact }];
    let started = app.start_run_blocking(request).expect("exact evidence");
    let captured = &started.data.run.run.exact_sources;
    assert_eq!(captured.len(), 1);
    assert_eq!(
        (captured[0].asset, captured[0].commit.as_str()),
        (asset, commit.as_str())
    );
    assert_eq!(captured[0].artifact, Some(exact));
}
