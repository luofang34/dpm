use super::*;
use crate::{CommandRequest, PlanChangeRequest};
use std::time::Duration;

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn request(app: &Application, actor: &str, command: Command, id: OperationId) -> CommandRequest {
    CommandRequest {
        actor: ActorId::agent(actor),
        base_revision: app.plan_blocking().expect("plan").revision,
        base_lineage: None,
        operation_id: Some(id),
        command,
    }
}

fn json(value: &RecordedOperation) -> serde_json::Value {
    serde_json::to_value(value).expect("json")
}

#[test]
fn a_resent_identity_returns_the_recorded_operation_and_other_content_is_refused() {
    let mut app = Application::in_memory_blocking(&fixture()).expect("app");
    let work = app.work_id_blocking("TEST-A").expect("work");
    let id = OperationId::new();
    let claim = request(&app, "worker", Command::Claim { work }, id);
    let first = app.execute_blocking(claim.clone()).expect("claim");
    assert_eq!(first.operation.id, id);
    // The resend carries the revision the first attempt has since moved past.
    let again = app.execute_blocking(claim.clone()).expect("resend");
    assert_eq!(json(&again), json(&first));
    assert_eq!(app.plan_blocking().expect("plan").revision, 1);

    let start = CommandRequest {
        command: Command::Start {
            work,
            occurred_at: None,
        },
        ..claim.clone()
    };
    let refused = app.execute_blocking(start).expect_err("other content");
    assert_eq!(refused.code(), "duplicate_operation");
    let details = refused.details().expect("details");
    assert_eq!(details["recorded"], json(&first));
    let impostor = CommandRequest {
        actor: ActorId::agent("impostor"),
        ..claim
    };
    assert_eq!(
        app.execute_blocking(impostor).expect_err("actor").code(),
        "duplicate_operation"
    );
    assert_eq!(app.plan_blocking().expect("plan").revision, 1);
}

#[test]
fn only_version_7_identities_are_accepted_and_absent_ones_are_minted() {
    let mut app = Application::in_memory_blocking(&fixture()).expect("app");
    let work = app.work_id_blocking("TEST-A").expect("work");
    let random = OperationId(dpm_model::WorkspaceId::new().0);
    let refused = app
        .execute_blocking(request(&app, "worker", Command::Claim { work }, random))
        .expect_err("version 4");
    assert_eq!(refused.code(), "invalid_request");
    let minted = app
        .execute_blocking(CommandRequest {
            operation_id: None,
            ..request(&app, "worker", Command::Claim { work }, random)
        })
        .expect("minted");
    assert!(minted.operation.id.is_time_ordered());
}

#[test]
fn a_resent_plan_change_is_compared_with_the_state_it_was_applied_to() {
    let mut app = Application::in_memory_blocking(&fixture()).expect("app");
    let mut proposed = app.plan_blocking().expect("plan");
    proposed.workspace.name = "Renamed".into();
    let id = OperationId::new();
    let change = PlanChangeRequest {
        actor: ActorId::human("lead"),
        base_revision: 0,
        base_lineage: None,
        operation_id: Some(id),
        plan: Box::new(proposed.clone()),
        reason: "rename".into(),
    };
    let first = app
        .apply_plan_change_blocking(change.clone())
        .expect("apply");
    let work = app.work_id_blocking("TEST-A").expect("work");
    app.execute_blocking(request(
        &app,
        "worker",
        Command::Claim { work },
        OperationId::new(),
    ))
    .expect("later operation");
    let again = app
        .apply_plan_change_blocking(change.clone())
        .expect("resend");
    assert_eq!(json(&again), json(&first));
    proposed.workspace.name = "Renamed differently".into();
    let other = PlanChangeRequest {
        plan: Box::new(proposed),
        ..change
    };
    assert_eq!(
        app.apply_plan_change_blocking(other)
            .expect_err("other proposal")
            .code(),
        "duplicate_operation"
    );
}

#[test]
fn restores_start_a_new_lineage_and_still_answer_recorded_identities() {
    let dir = tempfile::tempdir().expect("directory");
    let (live, backup, restored) = (
        dir.path().join("live.sqlite"),
        dir.path().join("backup.sqlite"),
        dir.path().join("restored.sqlite"),
    );
    let mut app = Application::initialize_blocking(&live, &fixture()).expect("app");
    let source = app.lineage_blocking().expect("lineage").expect("store");
    let work = app.work_id_blocking("TEST-A").expect("work");
    let claim = request(&app, "worker", Command::Claim { work }, OperationId::new());
    let first = app.execute_blocking(claim.clone()).expect("claim");
    assert_eq!(first.lineage_id, source);
    app.backup_blocking(&backup).expect("backup");
    let start = request(
        &app,
        "worker",
        Command::Start {
            work,
            occurred_at: None,
        },
        OperationId::new(),
    );
    let mut archive = Application::open_blocking(&backup).expect("archive");
    assert_eq!(
        archive
            .execute_blocking(start.clone())
            .expect_err("archive")
            .code(),
        "archived_store"
    );
    let report = crate::restore_store_blocking(&backup, &restored).expect("restore");
    let mut copy = Application::open_blocking(&restored).expect("restored");
    assert_eq!(
        copy.lineage_blocking().expect("lineage"),
        Some(report.lineage_id)
    );
    assert_ne!(report.lineage_id, source);
    let again = copy.execute_blocking(claim).expect("resend after restore");
    assert_eq!(
        json(&again),
        json(&first),
        "recorded under the source lineage"
    );
    let stale = CommandRequest {
        base_lineage: Some(source),
        ..start.clone()
    };
    let refused = copy.execute_blocking(stale).expect_err("other lineage");
    assert_eq!(refused.code(), "lineage_mismatch");
    assert_eq!(
        refused.details().expect("details")["actual"],
        serde_json::to_value(report.lineage_id).expect("json")
    );
    let started = copy
        .execute_blocking(CommandRequest {
            base_lineage: Some(report.lineage_id),
            ..start
        })
        .expect("observed lineage");
    assert_eq!(started.lineage_id, report.lineage_id);
}

#[test]
fn a_store_locked_past_its_timeout_answers_a_retryable_busy_code() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("live.sqlite");
    let mut app = Application::initialize_blocking(&path, &fixture()).expect("app");
    if let Backing::Database(store) = &app.backing {
        store
            .set_busy_timeout_blocking(Duration::from_millis(50))
            .expect("timeout");
    }
    let holder = rusqlite::Connection::open(&path).expect("raw connection");
    holder.execute_batch("BEGIN IMMEDIATE").expect("lock");
    let work = app.work_id_blocking("TEST-A").expect("work");
    let claim = request(&app, "worker", Command::Claim { work }, OperationId::new());
    let refused = app.execute_blocking(claim.clone()).expect_err("busy");
    assert_eq!(refused.code(), "store_busy", "{refused}");
    holder.execute_batch("ROLLBACK").expect("unlock");
    app.execute_blocking(claim)
        .expect("the same identity succeeds on retry");
}
