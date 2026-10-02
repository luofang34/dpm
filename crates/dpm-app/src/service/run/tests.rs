//! Runs through the application boundary: isolation from the project, idempotency, freshness and
//! the explicit link.

use super::*;
use crate::{CommandRequest, Query, QueryClock};
use chrono::{DateTime, TimeDelta, Utc};
use dpm_engine::Command;
use dpm_model::{
    ActivityInput, ActivityKind, ActorId, Observation, ObservedStatus, Plan, RunEventId, RunId,
    RunSession, RunState, WorkItemId, WorkStatus,
};

mod archives;
mod behavior;
mod boundaries;
mod files;

pub(super) fn worker() -> ActorId {
    ActorId::agent("worker")
}

pub(super) fn fixture_plan() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

/// Apply one project command as `actor` at the current revision.
pub(super) fn project(
    app: &mut Application,
    actor: &ActorId,
    command: Command,
) -> dpm_store::RecordedOperation {
    let base_revision = app.revision_blocking().expect("revision").revision;
    app.execute_blocking(CommandRequest {
        actor: actor.clone(),
        base_revision,
        base_lineage: None,
        operation_id: None,
        command,
    })
    .expect("project command")
}

pub(super) fn work(app: &Application) -> WorkItemId {
    app.work_id_blocking("TEST-A").expect("work")
}

/// An in-memory workspace whose first task the worker has claimed and started.
pub(super) fn started() -> Application {
    let mut app = Application::in_memory_blocking(&fixture_plan()).expect("app");
    let work = work(&app);
    project(&mut app, &worker(), Command::Claim { work });
    project(
        &mut app,
        &worker(),
        Command::Start {
            work,
            occurred_at: None,
        },
    );
    app
}

pub(super) fn start_request() -> RunStartRequest {
    RunStartRequest {
        actor: worker(),
        work_key: "TEST-A".into(),
        executor: None,
        run_id: Some(RunId::new()),
        parent: None,
        session: None,
        observation: Observation::ReportedOnly,
        sources: Vec::new(),
        observed_at: None,
        base_lineage: None,
    }
}

pub(super) fn start(app: &mut Application) -> RunId {
    let written = app.start_run_blocking(start_request()).expect("run");
    written.data.run.run.id
}

pub(super) fn report(run: RunId, state: RunState) -> RunReportRequest {
    RunReportRequest {
        actor: worker(),
        run,
        state,
        event_id: Some(RunEventId::new()),
        detail: None,
        observed_at: None,
        base_lineage: None,
    }
}

pub(super) fn activity(run: RunId, source_sequence: u64, kind: ActivityKind) -> RunActivityRequest {
    RunActivityRequest {
        actor: worker(),
        entries: vec![ActivityInput {
            run,
            source_sequence,
            kind,
            text: None,
            observed_at: None,
        }],
        base_lineage: None,
    }
}

fn at(app: &mut Application, when: DateTime<Utc>) {
    app.set_query_clock(QueryClock::Fixed(when));
}

fn code(error: AppError) -> &'static str {
    error.code()
}

#[test]
fn heartbeats_and_run_transitions_leave_the_project_exactly_as_it_was() {
    let mut app = started();
    let watcher = app.watch_commits();
    let before = app.plan_blocking().expect("plan");
    let history = app.history_blocking(0, 100).expect("history").entries.len();
    let run = start(&mut app);
    for index in 0..20_u64 {
        app.record_run_activity_blocking(activity(run, index + 1, ActivityKind::Heartbeat))
            .expect("heartbeat");
    }
    app.record_run_activity_blocking(activity(run, 21, ActivityKind::ToolResult))
        .expect("tool output");
    app.report_run_blocking(report(run, RunState::Waiting))
        .expect("waiting");
    app.report_run_blocking(report(run, RunState::Working))
        .expect("working");
    let after = app.plan_blocking().expect("plan");
    assert_eq!(
        after, before,
        "the plan, revision and ownership are untouched"
    );
    assert_eq!(after.revision, before.revision);
    assert_eq!(
        app.history_blocking(0, 100).expect("history").entries.len(),
        history
    );
    assert_eq!(
        app.revision_blocking().expect("revision").revision,
        before.revision
    );
    assert!(
        watcher.try_recv().is_err(),
        "no commit notification for a run write"
    );
    let item = &after.work_items[&work(&app)];
    assert_eq!(item.execution.status, WorkStatus::InProgress);
    assert_eq!(item.execution.owner, Some(worker()));
}

#[test]
fn a_completed_run_leaves_submitted_work_awaiting_its_independent_verifier() {
    let mut app = started();
    let work = work(&app);
    let run = start(&mut app);
    project(
        &mut app,
        &worker(),
        Command::Submit {
            work,
            note: Some("acceptance run".into()),
            occurred_at: None,
        },
    );
    let finished = app
        .report_run_blocking(report(run, RunState::Completed))
        .expect("complete");
    assert_eq!(finished.data.run.status, ObservedStatus::Completed);
    let item = &app.plan_blocking().expect("plan").work_items[&work];
    assert_eq!(
        item.execution.status,
        WorkStatus::Submitted,
        "completion is not acceptance"
    );
    assert_eq!(item.execution.owner, Some(worker()));
    assert!(item.execution.events.verified_at.is_none());
    // The run's executor cannot verify its own result, and the independent reviewer can.
    let base_revision = app.revision_blocking().expect("revision").revision;
    let verify = |actor| CommandRequest {
        actor,
        base_revision,
        base_lineage: None,
        operation_id: None,
        command: Command::Verify {
            work,
            note: None,
            occurred_at: None,
        },
    };
    assert!(app.execute_blocking(verify(worker())).is_err());
    app.execute_blocking(verify(ActorId::human("reviewer")))
        .expect("independent verification");
    let item = &app.plan_blocking().expect("plan").work_items[&work];
    assert_eq!(item.execution.status, WorkStatus::Verified);
}

#[test]
fn a_failed_or_interrupted_run_keeps_the_owner_and_the_started_work() {
    for end in [RunState::Failed, RunState::Interrupted] {
        let mut app = started();
        let run = start(&mut app);
        let mut request = report(run, end);
        request.detail = Some("the process exited".into());
        let view = app.report_run_blocking(request).expect("end").data.run;
        assert_eq!(view.state, end);
        assert_eq!(view.state_detail.as_deref(), Some("the process exited"));
        let item = &app.plan_blocking().expect("plan").work_items[&work(&app)];
        assert_eq!(item.execution.status, WorkStatus::InProgress, "{end}");
        assert_eq!(
            item.execution.owner,
            Some(worker()),
            "{end} must not release the claim"
        );
        assert!(item.execution.releases.is_empty());
    }
}

#[test]
fn a_task_has_many_runs_and_each_records_the_contract_it_observed() {
    let mut app = started();
    let first = start(&mut app);
    app.report_run_blocking(report(first, RunState::Failed))
        .expect("fail");
    let second = start(&mut app);
    // A reviewed plan change to other work moves the revision between the second and third run.
    let mut edited = app.export_blocking().expect("export").data;
    let id = work(&app);
    let other = edited
        .find_work_by_key("TEST-F")
        .map(|item| item.id)
        .expect("other work");
    edited.work_items.get_mut(&other).expect("work").title = "Clarified title".into();
    app.apply_plan_change_blocking(crate::PlanChangeRequest {
        actor: ActorId::human("lead"),
        base_revision: edited.revision,
        base_lineage: None,
        operation_id: None,
        plan: Box::new(edited),
        reason: "Clarify".into(),
    })
    .expect("plan change");
    let third = start(&mut app);
    let list = app
        .runs_blocking(&RunQuery {
            key: Some("TEST-A".into()),
            limit: 10,
        })
        .expect("runs")
        .data
        .runs;
    let ids: Vec<_> = list.iter().map(|view| view.run.id).collect();
    assert_eq!(ids, [third, second, first], "newest first, one task");
    assert!(list.iter().all(|view| view.run.work == id));
    let plan = app.plan_blocking().expect("plan");
    for view in &list {
        assert_eq!(view.run.contract.contract, plan.work_items[&id].contract);
        assert_eq!(view.run.contract.workspace_id, plan.workspace.id);
        assert_eq!(view.run.contract.prior_submissions, 0);
    }
    assert_eq!(list[1].run.contract.revision, list[2].run.contract.revision);
    assert!(list[0].run.contract.revision > list[1].run.contract.revision);
    assert_eq!(list[0].run.contract.work_key.0, "TEST-A");
}

#[test]
fn exact_retries_are_answered_and_changed_content_under_one_identity_is_refused() {
    let mut app = started();
    let request = start_request();
    let first = app.start_run_blocking(request.clone()).expect("start");
    assert!(!first.data.replayed);
    let again = app.start_run_blocking(request.clone()).expect("retry");
    assert!(again.data.replayed);
    assert_eq!(again.data.run.run, first.data.run.run);
    // Even after the task moved on, the retry answers with the recorded run.
    let work = work(&app);
    project(
        &mut app,
        &worker(),
        Command::Submit {
            work,
            note: None,
            occurred_at: None,
        },
    );
    assert!(
        app.start_run_blocking(request.clone())
            .expect("late retry")
            .data
            .replayed
    );
    let changed = RunStartRequest {
        session: Some(RunSession {
            provider: "codex".into(),
            session: "thread".into(),
            turn: None,
        }),
        ..request.clone()
    };
    let refused = app.start_run_blocking(changed).expect_err("changed");
    assert_eq!(code(refused), "duplicate_run_record");
    let run = request.run_id.expect("id");
    let transition = report(run, RunState::Waiting);
    assert!(
        !app.report_run_blocking(transition.clone())
            .expect("report")
            .data
            .replayed
    );
    assert!(
        app.report_run_blocking(transition.clone())
            .expect("retry")
            .data
            .replayed
    );
    let changed = RunReportRequest {
        state: RunState::Failed,
        ..transition
    };
    assert_eq!(
        code(app.report_run_blocking(changed).expect_err("changed")),
        "duplicate_run_record"
    );
    let heartbeat = activity(run, 1, ActivityKind::Heartbeat);
    let first = app
        .record_run_activity_blocking(heartbeat.clone())
        .expect("first");
    let second = app
        .record_run_activity_blocking(heartbeat)
        .expect("duplicate delivery");
    assert!(!first.data.entries[0].duplicate && second.data.entries[0].duplicate);
    assert_eq!(first.data.entries[0].entry, second.data.entries[0].entry);
    let lifecycle = app
        .run_lifecycle_blocking(0, 100, Some(run))
        .expect("feed")
        .data;
    assert_eq!(
        lifecycle.entries.len(),
        2,
        "the retries recorded no lifecycle fact"
    );
}
