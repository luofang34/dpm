#![allow(clippy::expect_used)]
use crate::{AppError, Application, CommandRequest, Query};
use dpm_engine::Command;
use dpm_model::{ActorId, Plan};

const RELEASE: &str = include_str!("../../../dpm-interchange/tests/mspdi/mpxj-release-plan.xml");

fn application() -> Application {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    Application::in_memory_blocking(&plan).expect("app")
}

fn import(app: &Application, xml: &str, project_key: &str) -> Result<serde_json::Value, AppError> {
    app.query_blocking(Query::ImportMspdi {
        xml: xml.into(),
        project_key: project_key.into(),
        key_prefix: Some("MSP".into()),
    })
    .map(|response| response.data)
}

/// Revision and number of recorded operations.
fn state(app: &Application) -> (u64, usize) {
    let page = app
        .query_blocking(Query::History {
            after_sequence: 0,
            limit: 1000,
        })
        .expect("history");
    let entries = page.data["entries"].as_array().expect("entries").len();
    (page.revision, entries)
}

fn apply(
    app: &mut Application,
    actor: ActorId,
    base: u64,
    candidate: &serde_json::Value,
) -> Result<(), AppError> {
    let plan: Plan = serde_json::from_value(candidate.clone()).expect("candidate");
    app.execute_blocking(CommandRequest {
        actor,
        base_revision: base,
        command: Command::ApplyChange {
            plan: Box::new(plan),
            reason: "Import the release schedule".into(),
        },
    })
    .map(|_| ())
}

#[test]
fn failed_imports_and_refused_applies_leave_revision_and_history_unchanged() {
    let mut app = application();
    let before = app.plan_blocking().expect("plan");
    for (xml, project, code) in [
        ("<not-closed", "TEST", "invalid_request"),
        ("<Project/>", "TEST", "invalid_request"),
        (RELEASE, "NOPE", "not_found"),
    ] {
        let error = import(&app, xml, project).expect_err("import must fail");
        assert_eq!(error.code(), code, "{error}");
        assert_eq!(state(&app), (0, 0));
    }
    let data = import(&app, RELEASE, "TEST").expect("import");
    assert_eq!(data["preview"]["base_revision"], 0);
    assert_eq!(state(&app), (0, 0));
    let candidate = &data["candidate"];
    let refused = apply(&mut app, ActorId::agent("importer"), 0, candidate).expect_err("agent");
    assert_eq!(refused.code(), "invalid_command");
    let stale = apply(&mut app, ActorId::human("reviewer"), 1, candidate).expect_err("stale");
    assert_eq!(stale.code(), "revision_conflict");
    assert_eq!(state(&app), (0, 0));
    assert_eq!(app.plan_blocking().expect("plan"), before);
    apply(&mut app, ActorId::human("reviewer"), 0, candidate).expect("apply");
    assert_eq!(state(&app), (1, 1));
    let again = import(&app, RELEASE, "TEST").expect("re-import");
    assert_eq!(again["preview"]["changes"], serde_json::json!([]));
}

#[test]
fn refusals_of_started_work_name_the_source_task_and_attempted_change() {
    let mut app = application();
    let work = app.work_id_blocking("TEST-A").expect("task");
    app.execute_blocking(CommandRequest {
        actor: ActorId::agent("worker"),
        base_revision: 0,
        command: Command::Claim { work },
    })
    .expect("claim");
    let exported = app
        .query_blocking(Query::ExportMspdi {
            project_key: "TEST".into(),
        })
        .expect("export")
        .data;
    let xml = exported["xml"].as_str().expect("xml").replace(
        "<Name>Contract A</Name>",
        "<Name>Contract A, renamed</Name>",
    );
    let error = import(&app, &xml, "TEST").expect_err("claimed work is protected");
    assert_eq!(error.code(), "invalid_command");
    let message = error.to_string();
    for expected in [
        "UID 1",
        "00000007-0000-4000-8000-000000000001",
        "TEST-A",
        "title",
        "Contract A, renamed",
        "protected",
    ] {
        assert!(
            message.to_uppercase().contains(&expected.to_uppercase()),
            "{expected} missing from: {message}"
        );
    }
    assert_eq!(state(&app), (1, 1));
}

#[test]
fn export_query_returns_the_document_and_report() {
    let app = application();
    let data = app
        .query_blocking(Query::ExportMspdi {
            project_key: "TEST".into(),
        })
        .expect("export")
        .data;
    let xml = data["xml"].as_str().expect("xml");
    assert!(xml.starts_with("<?xml"));
    assert_eq!(data["report"]["project"], "TEST");
    let reimport = import(&app, xml, "TEST").expect("re-import");
    assert_eq!(reimport["preview"]["changes"], serde_json::json!([]));
}
