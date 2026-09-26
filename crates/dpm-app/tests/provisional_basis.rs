//! Provisional bases survive restart and concurrent writers through the public application API.
#![allow(clippy::expect_used, clippy::panic)]

use dpm_app::{AppError, Application, CommandRequest, Query};
use dpm_engine::Command;
use dpm_model::{ActorId, DependencyId, Plan, StartBasis, WorkItemId};
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Copy)]
struct Ids {
    a: WorkItemId,
    b: WorkItemId,
    edge: DependencyId,
}

fn initialize(path: &Path) -> (Application, Ids) {
    let mut plan: Plan =
        serde_json::from_str(include_str!("../../../tests/support/execution-plan.json"))
            .expect("fixture");
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    let edge = plan
        .dependencies
        .iter_mut()
        .find(|d| d.predecessor == a && d.successor == b)
        .expect("A -> B");
    edge.start_basis = StartBasis::Provisional;
    let edge = edge.id;
    let app = Application::initialize_blocking(path, &plan).expect("initialize");
    (app, Ids { a, b, edge })
}

fn run(app: &mut Application, actor: &ActorId, command: Command) -> Result<u64, AppError> {
    let base_revision = app.plan_blocking().expect("plan").revision;
    app.execute_blocking(CommandRequest {
        actor: actor.clone(),
        base_revision,
        command,
    })
    .map(|operation| operation.resulting_revision)
}

fn ok(app: &mut Application, actor: &ActorId, command: Command) {
    let label = format!("{command:?}");
    run(app, actor, command).unwrap_or_else(|e| panic!("{label}: {e}"));
}

/// Every derived view an adapter can request for both tasks, at one revision.
fn views(app: &Application) -> Value {
    let query = |query| app.query_blocking(query).expect("query").data;
    serde_json::json!({
        "a": query(Query::Explain { key: "TEST-A".into() }),
        "b": query(Query::Explain { key: "TEST-B".into() }),
        "status": query(Query::Status { probabilistic: false }),
        "revision": app.plan_blocking().expect("plan").revision,
    })
}

fn flagged(views: &Value) -> Vec<Value> {
    views["b"]["transitions"]["verify"]["unmet"]
        .as_array()
        .expect("unmet")
        .iter()
        .filter(|g| g["type"] == "basis_invalidated")
        .cloned()
        .collect()
}

fn revalidate(ids: Ids, attempt: u32) -> Command {
    Command::RevalidateBasis {
        work: ids.b,
        dependency: ids.edge,
        attempt,
        reason: "B still matches A".into(),
    }
}

fn submit(work: WorkItemId) -> Command {
    Command::Submit { work, note: None }
}

/// B starts on A's first attempt, which is then rejected; returns every view at that point.
fn start_on_rejected_attempt(app: &mut Application, ids: Ids) -> Value {
    let (a, b) = (ids.a, ids.b);
    let decide = Command::Decide {
        decision: app.decision_id_blocking("TEST-GATE").expect("gate"),
        outcome: "Go".into(),
    };
    ok(app, &ActorId::human("reviewer"), decide);
    for command in [
        Command::Claim { work: a },
        Command::Start { work: a },
        submit(a),
    ] {
        ok(app, &ActorId::agent("author"), command);
    }
    for command in [Command::Claim { work: b }, Command::Start { work: b }] {
        ok(app, &ActorId::agent("builder"), command);
    }
    let reject = Command::Reject {
        work: a,
        reason: "fails acceptance".into(),
    };
    ok(app, &ActorId::human("reviewer"), reject);
    let rejected = views(app);
    let [gate] = flagged(&rejected).try_into().expect("B flagged");
    assert_eq!(
        (gate["attempt"].as_u64(), gate["current_attempt"].as_u64()),
        (Some(1), None)
    );
    assert_eq!(rejected["status"]["basis_invalidated"], 1);
    let [dependent] = rejected["a"]["basis"]["relied_on_by"]
        .as_array()
        .expect("dependents")
        .clone()
        .try_into()
        .expect("one");
    assert_eq!(
        (
            dependent["successor_key"].as_str(),
            dependent["state"]["state"].as_str()
        ),
        (Some("TEST-B"), Some("invalidated"))
    );
    rejected
}

#[test]
fn rejection_resubmission_and_revalidation_survive_restart_and_concurrent_writers() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("state.sqlite");
    let (mut app, ids) = initialize(&path);
    let reviewer = ActorId::human("reviewer");
    let rejected = start_on_rejected_attempt(&mut app, ids);
    drop(app);
    app = Application::open_blocking(&path).expect("reopen");
    assert_eq!(
        views(&app),
        rejected,
        "every view is identical after restart"
    );

    ok(&mut app, &ActorId::agent("author"), submit(ids.a));
    let verify = Command::Verify {
        work: ids.a,
        note: None,
    };
    ok(&mut app, &reviewer, verify);
    drop(app);
    app = Application::open_blocking(&path).expect("reopen");
    let verified = views(&app);
    let [gate] = flagged(&verified)
        .try_into()
        .expect("still flagged after A2 verified");
    assert_eq!(
        (gate["attempt"].as_u64(), gate["current_attempt"].as_u64()),
        (Some(1), Some(2))
    );

    let mut other = Application::open_blocking(&path).expect("second writer");
    let before = verified["revision"].as_u64().expect("revision");
    let stale = app.execute_blocking(CommandRequest {
        actor: reviewer.clone(),
        base_revision: before - 1,
        command: revalidate(ids, 2),
    });
    assert!(matches!(stale, Err(AppError::Conflict { .. })), "{stale:?}");
    let old_attempt = run(&mut app, &reviewer, revalidate(ids, 1)).expect_err("stale attempt");
    assert_eq!(old_attempt.code(), "invalid_command");
    let agent = ActorId::agent("builder");
    let by_agent = run(&mut app, &agent, revalidate(ids, 2)).expect_err("agent");
    assert_eq!(by_agent.code(), "invalid_command");
    assert_eq!(
        views(&app),
        verified,
        "refused revalidations change nothing"
    );

    let request = CommandRequest {
        actor: ActorId::service("ci"),
        base_revision: before,
        command: revalidate(ids, 2),
    };
    app.execute_blocking(request.clone()).expect("revalidate");
    let lost = other.execute_blocking(request);
    assert!(matches!(lost, Err(AppError::Conflict { .. })), "{lost:?}");
    drop((app, other));
    let revalidated = views(&Application::open_blocking(&path).expect("reopen"));
    assert!(flagged(&revalidated).is_empty());
    assert_eq!(revalidated["status"]["basis_invalidated"], 0);
    assert_eq!(revalidated["revision"].as_u64(), Some(before + 1));
    let basis = revalidated["b"]["work"]["basis"].as_array().expect("basis");
    let relied: Vec<_> = basis.iter().map(|x| x["attempt"].as_u64()).collect();
    assert_eq!(relied, [Some(1), Some(2)]);
    assert_eq!(basis[1]["source"]["kind"], "revalidation");
    let attempts = revalidated["a"]["work"]["attempts"]
        .as_array()
        .expect("attempts");
    assert_eq!(attempts[0]["outcome"]["state"], "rejected");
    assert_eq!(attempts[1]["outcome"]["state"], "verified");
}
