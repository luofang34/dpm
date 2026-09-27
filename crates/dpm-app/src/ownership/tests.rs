use crate::{Application, CommandRequest, Query};
use dpm_engine::Command;
use dpm_model::{ActorId, Plan};
use serde_json::json;

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn execute(app: &mut Application, actor: ActorId, command: Command) {
    let base_revision = app.plan_blocking().expect("plan").revision;
    let label = format!("{command:?}");
    app.execute_blocking(CommandRequest {
        actor,
        base_revision,
        command,
    })
    .unwrap_or_else(|e| panic!("{label}: {e}"));
}

#[test]
fn release_and_handoff_are_recorded_operations_that_survive_restart() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("state.sqlite");
    let mut app = Application::initialize_blocking(&path, &fixture()).expect("app");
    let work = app.work_id_blocking("TEST-A").expect("work");
    let (first, lead) = (ActorId::agent("first"), ActorId::human("lead"));
    execute(&mut app, first.clone(), Command::Claim { work });
    let release = app
        .release_command_blocking("TEST-A", "claimed by mistake".into())
        .expect("release");
    execute(&mut app, first.clone(), release);
    execute(&mut app, first.clone(), Command::Claim { work });
    execute(&mut app, first.clone(), Command::Start { work });
    let handoff = app
        .handoff_command_blocking("TEST-A", "agent:second", "first agent stopped".into())
        .expect("handoff");
    execute(&mut app, lead, handoff);
    drop(app);

    let reopened = Application::open_blocking(&path).expect("reopen");
    let history = reopened
        .query_blocking(Query::History {
            after_sequence: 0,
            limit: 100,
        })
        .expect("history")
        .data;
    let commands: Vec<_> = history["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|e| e["operation"]["command"].clone())
        .collect();
    assert_eq!(
        commands[1],
        json!({"Release": {"work": work, "reason": "claimed by mistake"}})
    );
    assert_eq!(
        commands[4],
        json!({"Handoff": {
            "work": work,
            "from": {"kind": "Agent", "name": "first"},
            "to": {"kind": "Agent", "name": "second"},
            "reason": "first agent stopped",
        }})
    );
    let shown = reopened
        .query_blocking(Query::Show {
            key: "TEST-A".into(),
        })
        .expect("show")
        .data;
    assert_eq!(
        shown["execution"]["owner"],
        json!({"kind": "Agent", "name": "second"})
    );
    assert_eq!(shown["execution"]["handoffs"][0]["actor"]["name"], "lead");
    assert!(shown["execution"]["events"]["started_at"].is_string());
}

#[test]
fn handoff_requests_are_refused_before_reaching_the_store() {
    let app = Application::in_memory_blocking(&fixture()).expect("app");
    let malformed = app
        .handoff_command_blocking("TEST-A", "second", "reason".into())
        .expect_err("actor spelling");
    assert_eq!(malformed.code(), "invalid_request");
    let unowned = app
        .handoff_command_blocking("TEST-A", "agent:second", "reason".into())
        .expect_err("no owner to hand off from");
    assert_eq!(unowned.code(), "invalid_command");
    let missing = app
        .release_command_blocking("NOPE", "reason".into())
        .expect_err("unknown key");
    assert_eq!(missing.code(), "not_found");
}
