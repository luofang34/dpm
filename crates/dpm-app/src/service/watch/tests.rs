use super::*;
use crate::{CommandRequest, PlanChangeRequest};
use dpm_engine::Command;
use dpm_model::{ActorId, OperationId, Plan};
use std::sync::{Arc, Mutex, mpsc::TryRecvError};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn claim(app: &Application, id: OperationId) -> CommandRequest {
    CommandRequest {
        actor: ActorId::agent("worker"),
        base_revision: 0,
        base_lineage: None,
        operation_id: Some(id),
        command: Command::Claim {
            work: app.work_id_blocking("TEST-A").expect("work"),
        },
    }
}

#[test]
fn every_commit_is_reported_once_and_nothing_else_is() {
    let mut app = Application::in_memory_blocking(&fixture()).expect("app");
    let lineage_id = app.lineage_blocking().expect("lineage");
    let commits = app.watch_commits();
    let request = claim(&app, OperationId::new());
    app.execute_blocking(request.clone()).expect("claim");
    assert_eq!(
        commits.try_recv(),
        Ok(WorkspaceRevision {
            revision: 1,
            lineage_id
        })
    );
    app.execute_blocking(request.clone()).expect("resend");
    let conflict = claim(&app, OperationId::new());
    app.execute_blocking(conflict).expect_err("stale revision");
    assert_eq!(
        commits.try_recv(),
        Err(TryRecvError::Empty),
        "no new commit"
    );

    let mut plan = app.plan_blocking().expect("plan");
    plan.workspace.name = "renamed".into();
    app.apply_plan_change_blocking(PlanChangeRequest {
        actor: ActorId::human("lead"),
        base_revision: 1,
        base_lineage: lineage_id,
        operation_id: None,
        plan: Box::new(plan),
        reason: "clearer name".into(),
    })
    .expect("plan change");
    assert_eq!(commits.try_recv().map(|r| r.revision), Ok(2));
    drop(commits);
    app.execute_blocking(CommandRequest {
        base_revision: 2,
        operation_id: None,
        command: Command::Start {
            work: app.work_id_blocking("TEST-A").expect("work"),
        },
        ..request
    })
    .expect("a dropped receiver does not fail commits");
    assert!(
        app.watchers.0.is_empty(),
        "the dropped receiver was discarded"
    );
}

fn assert_send<T: Send>() {}

#[test]
fn an_application_moves_between_threads_behind_a_mutex() {
    assert_send::<Application>();
    let mut app = Application::in_memory_blocking(&fixture()).expect("app");
    let commits = app.watch_commits();
    let request = claim(&app, OperationId::new());
    let shared = Arc::new(Mutex::new(app));
    let writer = Arc::clone(&shared);
    let handle = std::thread::spawn(move || {
        let mut app = writer.lock().expect("lock");
        app.execute_blocking(request)
            .map(|op| op.operation.resulting_revision)
    });
    let committed = handle.join().expect("thread").expect("claim");
    // The receiver lives on this thread; the commit on the other thread reached it.
    assert_eq!(commits.recv().map(|r| r.revision), Ok(committed));
    let app = shared.lock().expect("lock");
    assert_eq!(app.revision_blocking().expect("revision").revision, 1);
}
