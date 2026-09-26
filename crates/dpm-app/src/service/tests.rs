#![allow(clippy::expect_used)]
use super::*;

#[test]
fn revision_conflicts_and_independent_verification_are_atomic() {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let mut app = Application::in_memory_blocking(&plan).expect("app");
    let work = app.work_id_blocking("TEST-A").expect("work");
    let actor = ActorId::agent("worker");
    let request = CommandRequest {
        actor: actor.clone(),
        base_revision: 0,
        command: Command::Claim { work },
    };
    app.execute_blocking(request.clone()).expect("claim");
    assert!(matches!(
        app.execute_blocking(request),
        Err(AppError::Conflict { .. })
    ));
    app.execute_blocking(CommandRequest {
        actor: actor.clone(),
        base_revision: 1,
        command: Command::Submit {
            work,
            note: Some("acceptance passed".into()),
        },
    })
    .expect("submit");
    let verify = CommandRequest {
        actor,
        base_revision: 2,
        command: Command::Verify { work, note: None },
    };
    assert!(app.execute_blocking(verify.clone()).is_err());
    app.execute_blocking(CommandRequest {
        actor: ActorId::human("reviewer"),
        ..verify
    })
    .expect("verify");
    assert_eq!(app.plan_blocking().expect("plan").revision, 3);
    assert_eq!(
        app.query_blocking(Query::Show {
            key: "TEST-A".into()
        })
        .expect("show")
        .data["status"],
        "Verified"
    );
}
