#![allow(clippy::expect_used)]
use super::*;
use dpm_model::ActorId;
use std::collections::BTreeSet;

#[test]
fn revision_conflicts_and_independent_verification_are_atomic() {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("state.sqlite");
    let mut app = Application::initialize_blocking(&path, &plan).expect("app");
    let work = app.work_id_blocking("TEST-A").expect("work");
    let actor = ActorId::agent("worker");
    let request = CommandRequest {
        actor: actor.clone(),
        base_revision: 0,
        base_lineage: None,
        operation_id: None,
        command: Command::Claim { work },
    };
    app.execute_blocking(request.clone()).expect("claim");
    assert!(matches!(
        app.execute_blocking(request),
        Err(AppError::Conflict { .. })
    ));
    let submit = CommandRequest {
        actor: actor.clone(),
        base_revision: 1,
        base_lineage: None,
        operation_id: None,
        command: Command::Submit {
            work,
            note: Some("acceptance passed".into()),
        },
    };
    let refused = app
        .execute_blocking(submit.clone())
        .expect_err("claim is not start");
    assert!(refused.to_string().contains("has not started"), "{refused}");
    app.execute_blocking(CommandRequest {
        actor: actor.clone(),
        base_revision: 1,
        base_lineage: None,
        operation_id: None,
        command: Command::Start { work },
    })
    .expect("start");
    app.execute_blocking(CommandRequest {
        base_revision: 2,
        base_lineage: None,
        operation_id: None,
        ..submit
    })
    .expect("submit");
    let verify = CommandRequest {
        actor,
        base_revision: 3,
        base_lineage: None,
        operation_id: None,
        command: Command::Verify { work, note: None },
    };
    assert!(app.execute_blocking(verify.clone()).is_err());
    app.execute_blocking(CommandRequest {
        actor: ActorId::human("reviewer"),
        ..verify
    })
    .expect("verify");
    let recorded = app.plan_blocking().expect("plan").work_items[&work]
        .execution
        .events;
    drop(app);
    events_survive_restart(&path, work, recorded);
}

fn events_survive_restart(
    path: &std::path::Path,
    work: dpm_model::WorkItemId,
    recorded: dpm_model::ExecutionEvents,
) {
    let reopened = Application::open_blocking(path).expect("reopen");
    let plan = reopened.plan_blocking().expect("plan");
    assert_eq!(plan.revision, 4);
    assert_eq!(
        plan.work_items[&work].execution.events, recorded,
        "event facts survive restart"
    );
    assert!(recorded.started_at <= recorded.submitted_at);
    assert!(recorded.submitted_at <= recorded.verified_at && recorded.verified_at.is_some());
    assert_eq!(
        reopened
            .query_blocking(Query::Show {
                key: "TEST-A".into()
            })
            .expect("show")
            .data["execution"]["status"],
        "Verified"
    );
}

#[test]
fn refused_transitions_carry_the_same_unmet_gates_as_explain() {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let mut app = Application::in_memory_blocking(&plan).expect("app");
    let work = app.work_id_blocking("TEST-B").expect("work");
    let error = app
        .execute_blocking(CommandRequest {
            actor: ActorId::agent("worker"),
            base_revision: 0,
            base_lineage: None,
            operation_id: None,
            command: Command::Claim { work },
        })
        .expect_err("gated");
    let response = serde_json::to_value(error.response()).expect("json");
    let explained = app
        .query_blocking(Query::Explain {
            key: "TEST-B".into(),
        })
        .expect("explain")
        .data;
    assert_eq!(response["details"]["transition"], "claim");
    assert_eq!(response["details"]["unmet"], explained["gates"]["unmet"]);
    assert_eq!(app.plan_blocking().expect("plan").revision, 0);
}

fn next_query(project_keys: &[&str], asset_keys: &[&str], limit: usize) -> Query {
    Query::Next {
        capabilities: BTreeSet::new(),
        probabilistic: false,
        limit,
        project_keys: project_keys.iter().map(|k| k.to_string()).collect(),
        asset_keys: asset_keys.iter().map(|k| k.to_string()).collect(),
    }
}

#[test]
fn scoped_next_is_versioned_rejects_unknown_keys_and_leaves_state_unchanged() {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let app = Application::in_memory_blocking(&plan).expect("app");
    let before = app.query_blocking(Query::Export).expect("export").data;
    let scoped = app
        .query_blocking(next_query(&["TEST"], &["TEST-REPO"], 5))
        .expect("scoped")
        .data;
    assert_eq!(scoped["result_version"], dpm_engine::NEXT_RESULT_VERSION);
    assert_eq!(scoped["scope"]["projects"][0]["key"], "TEST");
    assert_eq!(scoped["candidates"][0]["work"]["key"], "TEST-A");
    assert_eq!(scoped["candidates"][0]["global_rank"], 1);
    assert_eq!(scoped["outside_scope"]["count"], 0);
    for query in [
        next_query(&["MISSING"], &[], 5),
        next_query(&[], &["MISSING"], 5),
    ] {
        let error = app.query_blocking(query).expect_err("unknown key");
        assert_eq!(error.code(), "not_found");
    }
    let after = app.query_blocking(Query::Export).expect("export").data;
    assert_eq!(after, before);
}

#[test]
fn next_requests_without_scope_fields_remain_accepted() {
    let query: Query = serde_json::from_value(serde_json::json!(
        {"query": "next", "capabilities": [], "probabilistic": false, "limit": 1}
    ))
    .expect("request without scope");
    assert!(matches!(
        query,
        Query::Next { ref project_keys, ref asset_keys, .. }
            if project_keys.is_empty() && asset_keys.is_empty()
    ));
}
