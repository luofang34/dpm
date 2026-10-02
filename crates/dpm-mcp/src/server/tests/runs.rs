//! The run tools: observations beside the plan, with no project revision involved.

use super::*;

fn project(server: &mut McpServer, name: &str, revision: u64) -> Value {
    call(
        server,
        name,
        json!({"key":"TEST-A","base_revision":revision}),
    )
}

/// A server whose worker has claimed and started the first task; returns the plan revision.
fn started() -> (McpServer, u64) {
    let mut server = server();
    initialize(&mut server);
    assert_eq!(project(&mut server, "claim_work", 0)["isError"], false);
    assert_eq!(project(&mut server, "start_work", 1)["isError"], false);
    (server, 2)
}

#[test]
fn run_tools_take_no_base_revision_or_operation_id_and_declare_their_cli_command() {
    let tools = crate::tools::definitions();
    let run_tools: Vec<_> = tools
        .iter()
        .filter(|tool| {
            tool["_meta"]["dpm/cli"]
                .as_str()
                .is_some_and(|command| command.starts_with("run "))
        })
        .collect();
    assert_eq!(run_tools.len(), 8);
    for tool in run_tools {
        let properties = &tool["inputSchema"]["properties"];
        let read_only = tool["annotations"]["readOnlyHint"] == true;
        assert!(properties.get("base_revision").is_none(), "{tool}");
        assert!(properties.get("operation_id").is_none(), "{tool}");
        assert_eq!(
            properties.get("base_lineage").is_some(),
            !read_only,
            "{tool}"
        );
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    }
}

#[test]
fn a_run_is_observed_through_the_tools_without_moving_the_project_revision() {
    let (mut server, revision) = started();
    let run = "0192f000-0000-7000-8000-0000000000a1";
    let started = call(
        &mut server,
        "start_run",
        json!({"key":"TEST-A","run_id":run,"session":{"provider":"codex","session":"thread-1"}}),
    );
    assert_eq!(started["isError"], false, "{started}");
    let content = &started["structuredContent"];
    assert_eq!(
        content["revision"], revision,
        "a run write reports the unchanged revision"
    );
    assert_eq!(content["data"]["replayed"], false);
    assert_eq!(content["data"]["run"]["status"], "working");
    assert_eq!(
        content["data"]["run"]["run"]["observation"],
        "reported_only"
    );
    let heartbeat = call(
        &mut server,
        "record_run_activity",
        json!({"run":run,"source_sequence":1,"kind":"heartbeat"}),
    );
    assert_eq!(
        heartbeat["structuredContent"]["data"]["entries"][0]["duplicate"],
        false
    );
    let again = call(
        &mut server,
        "record_run_activity",
        json!({"run":run,"source_sequence":1,"kind":"heartbeat"}),
    );
    assert_eq!(
        again["structuredContent"]["data"]["entries"][0]["duplicate"],
        true
    );
    let finished = call(
        &mut server,
        "report_run",
        json!({"run":run,"state":"completed"}),
    );
    assert_eq!(
        finished["structuredContent"]["data"]["run"]["status"],
        "completed"
    );
    let shown = call(&mut server, "get_run", json!({"run":run}));
    assert_eq!(shown["structuredContent"]["data"]["state"], "completed");
    let listed = call(&mut server, "list_runs", json!({"key":"TEST-A"}));
    assert_eq!(
        listed["structuredContent"]["data"]["runs"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    let lifecycle = call(&mut server, "run_lifecycle", json!({}));
    assert_eq!(lifecycle["structuredContent"]["data"]["head_sequence"], 2);
    let activity = call(&mut server, "run_activity", json!({"after_sequence":0}));
    assert_eq!(
        activity["structuredContent"]["data"]["entries"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    // The project is exactly where the run tools found it, and the work still awaits submission.
    let status = call(&mut server, "workspace_revision", json!({}));
    assert_eq!(status["structuredContent"]["data"]["revision"], revision);
    let work = call(&mut server, "get_work", json!({"key":"TEST-A"}));
    assert_eq!(
        work["structuredContent"]["data"]["execution"]["status"],
        "InProgress"
    );
}

#[test]
fn run_tools_refuse_project_preconditions_and_unknown_runs_with_stable_codes() {
    let (mut server, _) = started();
    let run = "0192f000-0000-7000-8000-0000000000a2";
    for argument in [("base_revision", json!(2)), ("operation_id", json!(run))] {
        let refused = call(
            &mut server,
            "start_run",
            json!({"key":"TEST-A", argument.0: argument.1}),
        );
        assert_eq!(
            refused["structuredContent"]["code"], "invalid_request",
            "{refused}"
        );
    }
    let unknown = call(&mut server, "get_run", json!({"run":run}));
    assert_eq!(unknown["structuredContent"]["code"], "not_found");
    let invalid = call(&mut server, "report_run", json!({"run":run,"state":"done"}));
    assert_eq!(invalid["structuredContent"]["code"], "invalid_request");
    let wrong_lineage = call(
        &mut server,
        "start_run",
        json!({"key":"TEST-A","base_lineage":"0192f000-0000-7000-8000-0000000000ff"}),
    );
    assert_eq!(
        wrong_lineage["structuredContent"]["code"],
        "lineage_mismatch"
    );
}

#[test]
fn a_run_cannot_be_started_by_an_agent_that_does_not_own_the_task() {
    let mut server = server();
    initialize(&mut server);
    let refused = call(&mut server, "start_run", json!({"key":"TEST-A"}));
    assert_eq!(
        refused["structuredContent"]["code"], "invalid_command",
        "{refused}"
    );
    let managed = call(
        &mut server,
        "start_run",
        json!({"key":"TEST-A","observation":"managed"}),
    );
    assert_eq!(managed["isError"], true);
}

#[test]
fn activity_sequences_must_rise_and_the_whole_text_is_part_of_the_identity() {
    let (mut server, _) = started();
    let run = "0192f000-0000-7000-8000-0000000000a5";
    call(
        &mut server,
        "start_run",
        json!({"key":"TEST-A","run_id":run}),
    );
    let record = |sequence: u64, text: &str| json!({"run":run,"source_sequence":sequence,"kind":"tool_result","text":text});
    let prefix = "x".repeat(4096);
    let stored = call(
        &mut server,
        "record_run_activity",
        record(5, &format!("{prefix}A")),
    );
    assert_eq!(stored["isError"], false, "{stored}");
    let entry = &stored["structuredContent"]["data"]["entries"][0];
    assert_eq!(entry["truncated"], true);
    assert_eq!(entry["text_digest"].as_str().map(str::len), Some(64));
    let resent = call(
        &mut server,
        "record_run_activity",
        record(5, &format!("{prefix}A")),
    );
    assert_eq!(
        resent["structuredContent"]["data"]["entries"][0]["duplicate"],
        true
    );
    let changed = call(
        &mut server,
        "record_run_activity",
        record(5, &format!("{prefix}B")),
    );
    assert_eq!(changed["structuredContent"]["code"], "duplicate_run_record");
    // A lower sequence that is not held cannot be told from a retry that retention removed.
    let late = call(&mut server, "record_run_activity", record(3, "late"));
    assert_eq!(late["structuredContent"]["code"], "activity_expired");
    assert_eq!(late["structuredContent"]["details"]["high_water"], 5);
    let zero = call(&mut server, "record_run_activity", record(0, "zero"));
    assert_eq!(zero["structuredContent"]["code"], "invalid_request");
    let key = call(
        &mut server,
        "record_run_activity",
        json!({"run":run,"key":"hb-1","kind":"heartbeat"}),
    );
    assert_eq!(
        key["structuredContent"]["code"], "invalid_request",
        "the old key is gone"
    );
}
