//! The native boundary against the shared application: negotiation, identity, parity with the
//! CLI and tool envelopes, and typed refusals.

use super::*;
use crate::{Application, CommandRequest, Envelope, Query};
use chrono::{DateTime, Utc};
use dpm_engine::Command;
use dpm_model::{ActorId, Plan, WorkItemId};
use serde_json::{Value, json};
use std::path::Path;

mod anchors;
mod clock;
mod fixtures;
mod handover;
mod harness;
mod links;
mod recovery;
mod schedule;
mod sources;
mod views;

pub(super) fn fixture_plan() -> Plan {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    plan.decisions.clear();
    plan
}

/// The fixture with a half-hour start-to-start lag from TEST-A to TEST-B.
pub(super) fn lagged_plan() -> Plan {
    let mut plan = fixture_plan();
    let a = plan.find_work_by_key("TEST-A").expect("TEST-A").id;
    let b = plan.find_work_by_key("TEST-B").expect("TEST-B").id;
    let edge = plan
        .dependencies
        .iter_mut()
        .find(|edge| edge.predecessor == a && edge.successor == b)
        .expect("edge");
    edge.kind = dpm_model::DependencyKind::StartStart;
    edge.lag_hours = 0.5;
    plan
}

pub(super) fn worker() -> ActorId {
    ActorId::agent("worker")
}

/// One round trip through the JSON line boundary, as a host would drive it.
pub(super) fn exchange_blocking(app: &mut Application, call: Value) -> Value {
    let request = json!({"protocol": 1, "id": "t-1", "call": call});
    serde_json::from_str(&app.native_json_blocking(&request.to_string())).expect("a JSON line")
}

pub(super) fn ok(response: &Value) -> &Value {
    assert_eq!(response["ok"], true, "{response}");
    assert!(response.get("error").is_none());
    &response["result"]
}

pub(super) fn refusal(response: &Value) -> &Value {
    assert_eq!(response["ok"], false, "{response}");
    assert!(response.get("result").is_none());
    &response["error"]
}

pub(super) fn attach_blocking(app: &mut Application) -> Attached {
    let response = exchange_blocking(app, json!({"type": "attach"}));
    let NativeResult::Attached(attached) =
        serde_json::from_value::<NativeResponse>(response.clone())
            .expect("typed response")
            .result
            .expect("a result")
    else {
        panic!("not an attach result: {response}");
    };
    attached
}

pub(super) fn changes_blocking(
    app: &mut Application,
    since: Cursors,
    limit: Option<u16>,
) -> ChangesResult {
    let response = exchange_blocking(
        app,
        json!({"type": "changes", "since": since, "limit": limit}),
    );
    match serde_json::from_value::<NativeResponse>(response.clone())
        .expect("typed response")
        .result
    {
        Some(NativeResult::Changes(changes)) => changes,
        other => panic!("not a changes result: {other:?} from {response}"),
    }
}

pub(super) fn view_blocking(app: &mut Application, query: Value) -> View {
    let response = exchange_blocking(app, json!({"type": "query", "query": query}));
    match serde_json::from_value::<NativeResponse>(response.clone())
        .expect("typed response")
        .result
    {
        Some(NativeResult::View(view)) => view,
        other => panic!("not a view: {other:?} from {response}"),
    }
}

/// Commit a project command as `actor` at the application's current revision.
pub(super) fn commit_blocking(
    app: &mut Application,
    actor: &ActorId,
    command: Command,
) -> dpm_store::RecordedOperation {
    let observed = app.revision_blocking().expect("revision");
    app.execute_blocking(CommandRequest {
        actor: actor.clone(),
        base_revision: observed.revision,
        base_lineage: observed.lineage_id,
        operation_id: None,
        command,
    })
    .expect("command")
}

pub(super) fn task_blocking(app: &Application) -> WorkItemId {
    app.work_id_blocking("TEST-A").expect("task")
}

pub(super) fn live_blocking(directory: &Path) -> Application {
    Application::initialize_blocking(&directory.join("state.sqlite"), &fixture_plan()).expect("app")
}

pub(super) fn at(app: &mut Application, when: DateTime<Utc>) {
    app.set_query_clock(crate::QueryClock::Fixed(when));
}

#[test]
fn hello_picks_the_highest_common_protocol_and_states_capabilities() {
    let mut app = Application::in_memory_blocking(&fixture_plan()).expect("app");
    let response = exchange_blocking(&mut app, json!({"type": "hello", "protocols": [7, 1, 0]}));
    let hello = ok(&response);
    assert_eq!(hello["kind"], "hello");
    assert_eq!(hello["protocol"], 1);
    assert_eq!(hello["supported"], json!([1]));
    assert_eq!(hello["api_version"], crate::API_VERSION);
    let capabilities = &hello["capabilities"];
    let views: Vec<_> = capabilities["views"]
        .as_array()
        .expect("views")
        .iter()
        .map(|view| view["view"].as_str().expect("name"))
        .collect();
    assert_eq!(views, ["now", "live", "review", "detail", "gantt"]);
    assert_eq!(capabilities["commands"], true);
    // Provider-independent runs, with unmanaged sessions labelled and no steering offered.
    assert_eq!(
        capabilities["runs"]["observation"],
        json!(["managed", "reported_only"])
    );
    let unsupported: Vec<_> = capabilities["unsupported"]
        .as_array()
        .expect("unsupported")
        .iter()
        .map(|control| control["control"].as_str().expect("control"))
        .collect();
    for control in [
        "steer_run",
        "stop_run",
        "answer_input_request",
        "provider_session_control",
    ] {
        assert!(unsupported.contains(&control), "{control}");
    }
    assert_eq!(capabilities["limits"]["max_page"], 1000);
    assert_eq!(capabilities["limits"]["stale_after_seconds"], 300);
}

#[test]
fn an_unsupported_protocol_is_refused_with_what_is_supported_before_the_call_is_decoded() {
    let mut app = Application::in_memory_blocking(&fixture_plan()).expect("app");
    let response = exchange_blocking(&mut app, json!({"type": "hello", "protocols": [2, 3]}));
    let error = refusal(&response);
    assert_eq!(error["code"], "unsupported_protocol");
    assert_eq!(
        error["details"],
        json!({"offered": [2, 3], "supported": [1]})
    );
    // A newer client's request is refused for its version, not as malformed, even though this
    // build could not decode the call it names.
    let future = json!({"protocol": 2, "id": "f-1", "call": {"type": "subscribe", "since": "now"}});
    let response: Value =
        serde_json::from_str(&app.native_json_blocking(&future.to_string())).expect("line");
    assert_eq!(refusal(&response)["code"], "unsupported_protocol");
    assert_eq!(response["id"], "f-1");
    assert_eq!(refusal(&response)["details"]["supported"], json!([1]));
    assert_eq!(refusal(&response)["api_version"], crate::API_VERSION);
}

#[test]
fn malformed_requests_get_a_typed_refusal_and_never_a_panic() {
    let mut app = Application::in_memory_blocking(&fixture_plan()).expect("app");
    for line in [
        "not json",
        "[]",
        "{}",
        r#"{"protocol":1,"id":"x"}"#,
        r#"{"protocol":1,"id":"x","call":{"type":"nope"}}"#,
        r#"{"protocol":1,"id":"x","call":{"type":"attach","surprise":true}}"#,
        r#"{"protocol":1,"id":"x","call":{"type":"query","query":{"query":"no_such_query"}}}"#,
        r#"{"protocol":"one","id":"x","call":{"type":"attach"}}"#,
    ] {
        let response: Value =
            serde_json::from_str(&app.native_json_blocking(line)).expect("always a JSON line");
        assert_eq!(refusal(&response)["code"], "invalid_request", "{line}");
        assert_eq!(response["ok"], false);
    }
    // The correlation id survives when it can be read.
    let response: Value = serde_json::from_str(
        &app.native_json_blocking(r#"{"protocol":1,"id":"keep","call":{"type":"nope"}}"#),
    )
    .expect("line");
    assert_eq!(response["id"], "keep");
}

#[test]
fn attach_names_the_workspace_lineage_and_where_every_feed_stands() {
    let directory = tempfile::tempdir().expect("directory");
    let mut app = live_blocking(directory.path());
    let plan = app.plan_blocking().expect("plan");
    let lineage = app.lineage_blocking().expect("lineage");
    let before = attach_blocking(&mut app);
    assert_eq!(before.source, SourceKind::Live);
    assert_eq!(
        before.attachment,
        Attachment {
            workspace_id: plan.workspace.id,
            lineage_id: lineage
        }
    );
    assert_eq!(before.watermark.project.revision, plan.revision);
    assert_eq!(before.watermark.project.history_head, 0);
    assert_eq!(before.watermark.runs, dpm_model::RunFeedHeads::default());
    let task = task_blocking(&app);
    commit_blocking(&mut app, &worker(), Command::Claim { work: task });
    commit_blocking(
        &mut app,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    let after = attach_blocking(&mut app);
    assert_eq!(after.watermark.project.history_head, 2);
    assert_eq!(
        after.watermark.project.revision,
        app.revision_blocking().expect("revision").revision
    );
    // An expected workspace that is not this one is refused with both identities.
    let other = dpm_model::WorkspaceId::new();
    let response = exchange_blocking(
        &mut app,
        json!({"type": "attach", "expect_workspace": other}),
    );
    let error = refusal(&response);
    assert_eq!(error["code"], "workspace_mismatch");
    assert_eq!(error["details"]["expected"], json!(other));
    assert_eq!(error["details"]["actual"], json!(plan.workspace.id));
}

#[test]
fn a_preview_is_attachable_read_only_and_refuses_commands_with_the_shared_code() {
    let mut app = Application::preview(fixture_plan()).expect("preview");
    let attached = attach_blocking(&mut app);
    assert_eq!(attached.source, SourceKind::Preview);
    assert_eq!(attached.attachment.lineage_id, None);
    assert_eq!(attached.watermark.project.history_head, 0);
    let hello = exchange_blocking(&mut app, json!({"type": "hello", "protocols": [1]}));
    assert_eq!(ok(&hello)["capabilities"]["commands"], false);
    let task = fixture_plan().find_work_by_key("TEST-A").expect("task").id;
    let request = CommandRequest {
        actor: worker(),
        base_revision: 0,
        base_lineage: None,
        operation_id: None,
        command: Command::Claim { work: task },
    };
    let response = exchange_blocking(&mut app, json!({"type": "command", "request": request}));
    assert_eq!(refusal(&response)["code"], "read_only_project");
    // Queries answer, with no feeds to follow.
    let status = view_blocking(&mut app, json!({"query": "status", "probabilistic": false}));
    assert_eq!(
        status.basis.runs, None,
        "a project-only answer has no run basis"
    );
    assert!(status.basis.project.is_some());
}

#[test]
fn a_view_is_the_same_envelope_the_cli_and_tools_return_with_the_identity_to_follow_it() {
    let directory = tempfile::tempdir().expect("directory");
    let mut app = live_blocking(directory.path());
    let task = task_blocking(&app);
    commit_blocking(&mut app, &worker(), Command::Claim { work: task });
    let when: DateTime<Utc> = "2026-10-02T12:00:00Z".parse().expect("time");
    at(&mut app, when);
    for query in [
        json!({"query": "status", "probabilistic": false}),
        json!({"query": "next", "capabilities": [], "probabilistic": false, "limit": 5}),
        json!({"query": "explain", "key": "TEST-A"}),
        json!({"query": "show", "key": "TEST-B"}),
        json!({"query": "history", "after_sequence": 0, "limit": 10}),
        json!({"query": "runs"}),
    ] {
        let native = view_blocking(&mut app, query.clone());
        let parsed: Query = serde_json::from_value(query.clone()).expect("query");
        let shared = Envelope::from(app.query_blocking(parsed).expect("shared query"));
        assert_eq!(
            serde_json::to_value(&native.envelope).expect("json"),
            serde_json::to_value(&shared).expect("json"),
            "{query}"
        );
        assert_eq!(native.evaluated_at, when, "{query}");
        // The view names exactly the revision and lineage the watermark was read at.
        assert_eq!(
            native.envelope.revision,
            Some(harness::project_basis(&native).revision)
        );
        assert_eq!(
            native.envelope.lineage_id,
            harness::project_basis(&native).lineage_id
        );
    }
}

#[test]
fn commands_keep_the_applications_preconditions_and_typed_refusals() {
    let directory = tempfile::tempdir().expect("directory");
    let mut app = live_blocking(directory.path());
    let task = task_blocking(&app);
    let claim = |revision: u64| {
        json!({"type": "command", "request": CommandRequest {
            actor: worker(), base_revision: revision, base_lineage: None,
            operation_id: None, command: Command::Claim { work: task },
        }})
    };
    let response = exchange_blocking(&mut app, claim(0));
    let committed = ok(&response);
    assert_eq!(committed["kind"], "committed");
    assert_eq!(committed["envelope"]["revision"], 1);
    assert_eq!(committed["envelope"]["data"]["resulting_revision"], 1);
    // A stale revision, a domain refusal and a malformed command each keep their shared code.
    assert_eq!(
        refusal(&exchange_blocking(&mut app, claim(0)))["code"],
        "revision_conflict"
    );
    let submit = json!({"type": "command", "request": CommandRequest {
        actor: worker(), base_revision: 1, base_lineage: None, operation_id: None,
        command: Command::Submit { work: task, note: None, occurred_at: None },
    }});
    let refused = exchange_blocking(&mut app, submit);
    assert_eq!(refusal(&refused)["code"], "invalid_command");
    assert!(
        refusal(&refused)["message"]
            .as_str()
            .is_some_and(|m| m.contains("has not started"))
    );
    let malformed = exchange_blocking(
        &mut app,
        json!({"type": "command", "request": {"actor": "x"}}),
    );
    assert_eq!(refusal(&malformed)["code"], "invalid_request");
}

#[test]
fn a_backup_archive_is_attachable_readable_and_refuses_commands() {
    let directory = tempfile::tempdir().expect("directory");
    let app = live_blocking(directory.path());
    let archive = directory.path().join("archive.sqlite");
    app.backup_blocking(&archive).expect("backup");
    let mut archived = Application::open_blocking(&archive).expect("open");
    assert_eq!(attach_blocking(&mut archived).source, SourceKind::Archive);
    let hello = exchange_blocking(&mut archived, json!({"type": "hello", "protocols": [1]}));
    assert_eq!(ok(&hello)["capabilities"]["commands"], false);
    let task = task_blocking(&archived);
    let request = CommandRequest {
        actor: worker(),
        base_revision: 0,
        base_lineage: None,
        operation_id: None,
        command: Command::Claim { work: task },
    };
    let response = exchange_blocking(
        &mut archived,
        json!({"type": "command", "request": request}),
    );
    assert_eq!(refusal(&response)["code"], "archived_store");
    let status = view_blocking(
        &mut archived,
        json!({"query": "status", "probabilistic": false}),
    );
    assert_eq!(status.envelope.revision, Some(0));
}
