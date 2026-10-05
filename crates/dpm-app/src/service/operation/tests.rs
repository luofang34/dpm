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

/// Rewrites every whole floating-point number as an integer, as a client's JSON encoder may.
fn integral(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e15 => {
                serde_json::json!(f as i64)
            }
            _ => value.clone(),
        },
        serde_json::Value::Array(items) => items.iter().map(integral).collect(),
        serde_json::Value::Object(map) => {
            map.iter().map(|(k, v)| (k.clone(), integral(v))).collect()
        }
        other => other.clone(),
    }
}

#[test]
fn a_reviewed_change_sent_back_with_whole_numbers_applies_once_and_its_resend_is_answered() {
    let mut app = Application::in_memory_blocking(&fixture()).expect("app");
    let current = app.plan_blocking().expect("plan");
    let work = app.work_id_blocking("TEST-B").expect("work");
    let mut proposed = current.clone();
    if let Some(estimate) = proposed
        .work_items
        .get_mut(&work)
        .and_then(|w| w.schedule.estimate.as_mut())
    {
        estimate.likely_hours = 22.0;
    }
    let Command::ApplyChange { changes, reason } =
        dpm_engine::plan_change(&current, &proposed, "Widen B").expect("change")
    else {
        panic!("a plan change");
    };
    let reencoded: Vec<_> = changes
        .iter()
        .map(|c| dpm_engine::EntityChange {
            before: integral(&c.before),
            after: integral(&c.after),
            ..c.clone()
        })
        .collect();
    let id = OperationId::new();
    let sent = CommandRequest {
        actor: ActorId::human("planner"),
        base_revision: current.revision,
        base_lineage: None,
        operation_id: Some(id),
        command: Command::ApplyChange {
            changes: reencoded,
            reason: reason.clone(),
        },
    };
    let first = app
        .execute_blocking(sent.clone())
        .expect("a re-encoded reviewed change applies");
    let mut altered = sent.clone();
    if let Command::ApplyChange { changes, .. } = &mut altered.command {
        for change in changes.iter_mut() {
            change.after = integral(&change.after)
                .to_string()
                .replace("22", "23")
                .parse()
                .expect("json");
        }
    }
    let again = app
        .execute_blocking(sent)
        .expect("its resend is answered with the recorded operation");
    assert_eq!(
        app.execute_blocking(altered)
            .expect_err("a different value under the same identity")
            .code(),
        "duplicate_operation"
    );
    assert_eq!(json(&again), json(&first));
    assert_eq!(
        app.plan_blocking().expect("plan").revision,
        current.revision + 1
    );
    match &first.operation.command {
        Command::ApplyChange {
            changes: recorded, ..
        } => assert_eq!(recorded, &changes, "the log keeps the canonical form"),
        other => panic!("unexpected command {other:?}"),
    }
}
