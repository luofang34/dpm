#![allow(clippy::expect_used, clippy::panic)]
use crate::{AppError, Application, CommandRequest, Query};
use dpm_engine::Command;
use dpm_model::{ActorId, DependencyPolicy, Plan};

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
        match_existing_by: None,
        keep_existing_priority: false,
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
    app.apply_plan_change_blocking(crate::PlanChangeRequest {
        actor,
        base_revision: base,
        plan: Box::new(plan),
        reason: "Import the release schedule".into(),
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

/// Application whose soft TEST-A -> TEST-B edge a human has waived, with its exported document and
/// the exported UIDs of TEST-A and TEST-B.
fn waived_edge_export() -> (Application, String, u64, u64) {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let key = |plan: &Plan, key: &str| plan.find_work_by_key(key).expect(key).id;
    let (a, b) = (key(&plan, "TEST-A"), key(&plan, "TEST-B"));
    let edge = plan
        .dependencies
        .iter_mut()
        .find(|d| d.predecessor == a && d.successor == b)
        .expect("edge");
    edge.policy = DependencyPolicy::Soft;
    let dependency = edge.id;
    let mut app = Application::in_memory_blocking(&plan).expect("app");
    app.execute_blocking(CommandRequest {
        actor: ActorId::human("reviewer"),
        base_revision: 0,
        command: Command::WaiveDependency {
            dependency,
            reason: "Contract B may start early".into(),
        },
    })
    .expect("waive");
    let exported = app
        .query_blocking(Query::ExportMspdi {
            project_key: "TEST".into(),
        })
        .expect("export")
        .data;
    let uid = |key: &str| {
        exported["report"]["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|i| i["key"] == key)
            .and_then(|i| i["uid"].as_u64())
            .expect(key)
    };
    let (from, to) = (uid("TEST-A"), uid("TEST-B"));
    let xml = exported["xml"].as_str().expect("xml").to_owned();
    (app, xml, from, to)
}

/// Replace the first occurrence of `from` inside the task element with UID `uid`.
fn edit_task(xml: &str, uid: u64, from: &str, to: &str) -> String {
    let start = xml.find(&format!("<UID>{uid}</UID>")).expect("task");
    let end = start + xml[start..].find("</Task>").expect("task end");
    let task = xml[start..end].replacen(from, to, 1);
    assert_ne!(task, xml[start..end], "{from} not in task UID {uid}");
    format!("{}{task}{}", &xml[..start], &xml[end..])
}

fn assert_names_the_link(error: &AppError, from: u64, to: u64, attempted: &[&str]) {
    assert_eq!(error.code(), "invalid_command");
    let AppError::Interchange(dpm_interchange::InterchangeError::RefusedLink { source, .. }) =
        error
    else {
        panic!("not a link refusal: {error}");
    };
    assert!(source.to_string().contains("waived"), "{source}");
    let message = error.to_string();
    let uids = [format!("UID {from}"), format!("UID {to}")];
    let named = [
        "00000007-0000-4000-8000-000000000001",
        "00000007-0000-4000-8000-000000000002",
        "TEST-A",
        "TEST-B",
        "waived",
    ];
    for expected in uids
        .iter()
        .map(String::as_str)
        .chain(named)
        .chain(attempted.iter().copied())
    {
        assert!(
            message.to_uppercase().contains(&expected.to_uppercase()),
            "{expected} missing from: {message}"
        );
    }
}

#[test]
fn refusals_of_waived_edges_name_the_source_link_and_attempted_change() {
    let (app, xml, from, to) = waived_edge_export();
    let relag = edit_task(&xml, to, "<LinkLag>0</LinkLag>", "<LinkLag>1200</LinkLag>");
    let error = import(&app, &relag, "TEST").expect_err("waived edge is protected");
    assert_names_the_link(&error, from, to, &["change", "lag 0 h", "lag 2 h"]);
    let start = xml.find(&format!("<UID>{to}</UID>")).expect("task");
    let link = start + xml[start..].find("<PredecessorLink>").expect("link");
    let end =
        link + xml[link..].find("</PredecessorLink>").expect("end") + "</PredecessorLink>".len();
    let dropped = format!("{}{}", &xml[..link], &xml[end..]);
    let error = import(&app, &dropped, "TEST").expect_err("waived edge is protected");
    assert_names_the_link(&error, from, to, &["remove", "FS TEST-A -> TEST-B"]);
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
