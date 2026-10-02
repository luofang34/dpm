//! Synthetic maintenance exercises planning, observation and independent acceptance together.
#![cfg(feature = "sqlite")]
#![cfg(test)]

use chrono::{Duration, Utc};
use dpm_app::{Application, CommandRequest, PlanChangeRequest, QueryClock};
use dpm_engine::Command;
use dpm_model::{
    ActorId, Artifact, ArtifactId, ArtifactKind, DependencyId, DependencyKind, ExecutionRecord,
    Key, OperationId, Plan, SiblingOrder, WorkItemId, WorkStatus,
};
use std::{collections::BTreeMap, sync::mpsc::TryRecvError};

fn fixture() -> Plan {
    let mut plan: Plan =
        serde_json::from_str(include_str!("../../../tests/support/execution-plan.json"))
            .expect("synthetic plan");
    plan.decisions.clear();
    plan
}

fn request_blocking(app: &Application, actor: &ActorId, command: Command) -> CommandRequest {
    let observed = app.revision_blocking().expect("observed revision");
    CommandRequest {
        actor: actor.clone(),
        base_revision: observed.revision,
        base_lineage: observed.lineage_id,
        operation_id: Some(OperationId::new()),
        command,
    }
}

fn run_blocking(app: &mut Application, actor: &ActorId, command: Command) {
    let request = request_blocking(app, actor, command);
    app.execute_blocking(request).expect("accepted command");
}

fn refuse_blocking(app: &mut Application, actor: &ActorId, command: Command) {
    let before = app.history_blocking(0, 1000).expect("history");
    let request = request_blocking(app, actor, command);
    app.execute_blocking(request).expect_err("refused command");
    let after = app.history_blocking(0, 1000).expect("history");
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
}

fn ready_blocking(app: &Application, key: &str) -> bool {
    app.explain_blocking(key).expect("explanation").data.ready
}

fn propose_maintenance_blocking(app: &mut Application) -> WorkItemId {
    let mut plan = app.plan_blocking().expect("base plan");
    let a = plan.find_work_by_key("TEST-A").expect("downstream").id;
    let mut work = plan.work_items[&a].clone();
    work.id = WorkItemId::new();
    work.key = Key::new("MAINT-CHANGE");
    work.title = "Qualify a bounded synthetic maintenance change".into();
    work.contract.objective = "Produce the scoped regression evidence before dependent work".into();
    work.execution = ExecutionRecord::default();
    work.order = SiblingOrder(vec![50]);
    let id = work.id;
    plan.work_items.insert(id, work);
    let mut edge = plan.dependencies.first().expect("edge template").clone();
    edge.id = DependencyId::new();
    edge.predecessor = id;
    edge.successor = a;
    plan.dependencies.push(edge);
    let preview = app.propose_change_blocking(&plan).expect("proposal");
    assert!(!preview.data.changes.is_empty());
    assert_eq!(app.revision_blocking().unwrap().revision, 0);
    let mut request = PlanChangeRequest {
        actor: ActorId::agent("maintainer"),
        base_revision: 0,
        base_lineage: preview.lineage_id,
        operation_id: Some(OperationId::new()),
        plan: Box::new(plan),
        reason: "Review a bounded maintenance prerequisite".into(),
    };
    app.apply_plan_change_blocking(request.clone())
        .expect_err("agent cannot approve");
    request.actor = ActorId::human("planner");
    let first = app
        .apply_plan_change_blocking(request.clone())
        .expect("reviewed change");
    let resent = app
        .apply_plan_change_blocking(request)
        .expect("idempotent retry");
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(resent).unwrap()
    );
    assert_eq!(
        app.show_blocking("MAINT-CHANGE")
            .unwrap()
            .data
            .work
            .execution
            .status,
        WorkStatus::Proposed
    );
    assert!(!ready_blocking(app, "MAINT-CHANGE") && !ready_blocking(app, "TEST-A"));
    id
}

fn perform_maintenance_blocking(app: &mut Application, id: WorkItemId) {
    let agent = ActorId::agent("maintainer");
    refuse_blocking(app, &agent, Command::RatifyContract { work: id });
    run_blocking(
        app,
        &ActorId::human("planner"),
        Command::RatifyContract { work: id },
    );
    assert!(ready_blocking(app, "MAINT-CHANGE"));
    run_blocking(app, &agent, Command::Claim { work: id });
    refuse_blocking(app, &agent, submit(id));
    run_blocking(
        app,
        &agent,
        Command::Start {
            work: id,
            occurred_at: None,
        },
    );
    let mut rewritten = app.plan_blocking().expect("started plan");
    rewritten
        .work_items
        .get_mut(&id)
        .unwrap()
        .contract
        .objective = "Unreviewed different result".into();
    app.propose_change_blocking(&rewritten)
        .expect_err("started contract is protected");
    let artifact = Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::TestResult,
        uri: "test:synthetic-maintenance/check-output".into(),
        label: "Synthetic regression evidence, not self-host completion".into(),
        metadata: BTreeMap::from([("input".into(), "synthetic execution fixture".into())]),
        created_by: agent.clone(),
        created_at: Utc::now(),
    };
    run_blocking(app, &agent, Command::AttachArtifact { work: id, artifact });
    run_blocking(
        app,
        &agent,
        Command::ReportProgress {
            work: id,
            percent: 100,
            note: None,
        },
    );
    assert!(
        !ready_blocking(app, "TEST-A"),
        "execution report is not acceptance"
    );
    run_blocking(app, &agent, submit(id));
    refuse_blocking(app, &agent, verify(id));
    assert!(!ready_blocking(app, "TEST-A"));
}

fn submit(work: WorkItemId) -> Command {
    Command::Submit {
        work,
        note: Some("Synthetic result ready for review".into()),
        occurred_at: None,
    }
}

fn verify(work: WorkItemId) -> Command {
    Command::Verify {
        work,
        note: Some("Synthetic acceptance checked".into()),
        occurred_at: None,
    }
}

#[test]
fn maintenance_proposal_execution_and_review_are_observable_and_replayable() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("maintenance.sqlite");
    let mut writer = Application::initialize_blocking(&path, &fixture()).expect("store");
    let local_feed = writer.watch_commits();
    let mut observer = Application::open_blocking(&path).expect("observer");
    let observer_feed = observer.watch_commits();
    let id = propose_maintenance_blocking(&mut writer);
    assert_eq!(local_feed.try_recv().unwrap().revision, 1);
    assert_eq!(
        local_feed.try_recv(),
        Err(TryRecvError::Empty),
        "retry is not a commit"
    );
    assert_eq!(
        observer_feed.try_recv(),
        Err(TryRecvError::Empty),
        "external writes need a revision probe"
    );
    assert_eq!(observer.revision_blocking().unwrap().revision, 1);
    perform_maintenance_blocking(&mut writer, id);
    let submitted = observer
        .explain_blocking("MAINT-CHANGE")
        .expect("external result");
    assert_eq!(submitted.data.work.execution.status, WorkStatus::Submitted);
    assert_eq!(submitted.data.context.artifacts.len(), 1);
    let cursor = observer
        .history_blocking(0, 1000)
        .unwrap()
        .next_after_sequence;
    let reviewer = ActorId::human("independent-reviewer");
    run_blocking(
        &mut writer,
        &reviewer,
        Command::Reject {
            work: id,
            reason: "Missing edge-case assertion".into(),
        },
    );
    assert!(!ready_blocking(&observer, "TEST-A"));
    drop(writer);
    let mut reopened = Application::open_blocking(&path).expect("restart");
    run_blocking(&mut reopened, &ActorId::agent("maintainer"), submit(id));
    assert!(!ready_blocking(&observer, "TEST-A"));
    run_blocking(&mut reopened, &reviewer, verify(id));
    assert!(ready_blocking(&observer, "TEST-A"));
    let tail = observer
        .history_blocking(cursor, 1000)
        .expect("resume history");
    assert_eq!(tail.entries.len(), 3);
    let accepted = observer
        .explain_blocking("MAINT-CHANGE")
        .expect("reviewed result");
    assert_eq!(accepted.data.work.execution.attempts.len(), 2);
    assert!(accepted.data.work.execution.last_rejection.is_some());
    let before = observer.revision_blocking().unwrap();
    let report = dpm_app::verify_store_blocking(&path).expect("replay matches snapshot");
    assert_eq!(report.operation_count, tail.next_after_sequence);
    assert_eq!(observer.revision_blocking().unwrap(), before);
}

#[test]
fn elapsed_lag_changes_a_view_without_a_commit_notification() {
    let mut plan = fixture();
    let a = plan.find_work_by_key("TEST-A").unwrap().id;
    let b = plan.find_work_by_key("TEST-B").unwrap().id;
    let edge = plan
        .dependencies
        .iter_mut()
        .find(|e| e.predecessor == a && e.successor == b)
        .unwrap();
    edge.kind = DependencyKind::StartStart;
    edge.lag_hours = 0.5;
    let mut app = Application::in_memory_blocking(&plan).expect("store");
    let agent = ActorId::agent("maintainer");
    run_blocking(&mut app, &agent, Command::Claim { work: a });
    run_blocking(
        &mut app,
        &agent,
        Command::Start {
            work: a,
            occurred_at: None,
        },
    );
    let feed = app.watch_commits();
    let start = app
        .show_blocking("TEST-A")
        .unwrap()
        .data
        .work
        .execution
        .events
        .started_at
        .unwrap();
    let revision = app.revision_blocking().unwrap();
    app.set_query_clock(QueryClock::Fixed(start + Duration::seconds(1799)));
    assert!(!ready_blocking(&app, "TEST-B"));
    app.set_query_clock(QueryClock::Fixed(start + Duration::seconds(1800)));
    assert!(ready_blocking(&app, "TEST-B"));
    assert_eq!(app.revision_blocking().unwrap(), revision);
    assert_eq!(feed.try_recv(), Err(TryRecvError::Empty));
}
