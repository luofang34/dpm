use super::*;
use crate::serve_blocking;

fn server() -> McpServer {
    let plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("plan");
    McpServer::new(
        Application::in_memory_blocking(&plan).expect("app"),
        ActorId::agent("mcp-worker"),
    )
}
fn initialize(server: &mut McpServer) {
    let response = server.handle_blocking(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).expect("response");
    assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
    assert!(
        server
            .handle_blocking(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .is_none()
    );
}
#[test]
fn advertises_tools_and_preserves_revision_conflicts() {
    let mut server = server();
    initialize(&mut server);
    let list = server
        .handle_blocking(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        .expect("list");
    assert_eq!(list["result"]["tools"].as_array().expect("tools").len(), 35);
    let request = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"claim_work","arguments":{"key":"TEST-A","base_revision":0}}});
    assert_eq!(
        server.handle_blocking(request.clone()).expect("claim")["result"]["isError"],
        false
    );
    assert_eq!(
        server.handle_blocking(request).expect("stale")["result"]["structuredContent"]["code"],
        "revision_conflict"
    );
}
#[test]
fn newline_transport_recovers_from_parse_error() {
    let mut server = server();
    let input = b"invalid\n{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"ping\"}\n";
    let mut output = Vec::new();
    serve_blocking(&mut server, std::io::Cursor::new(input), &mut output).expect("transport");
    let text = String::from_utf8(output).expect("utf8");
    let messages: Vec<Value> = text
        .lines()
        .map(|s| serde_json::from_str(s).expect("json"))
        .collect();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["error"]["code"], -32700);
    assert_eq!(messages[1]["id"], 4);
}
#[test]
fn requires_initialization_and_rejects_bad_requests() {
    let mut server = server();
    assert_eq!(
        server
            .handle_blocking(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .expect("response")["error"]["code"],
        -32002
    );
    assert_eq!(
        server.handle_blocking(json!([])).expect("response")["error"]["code"],
        -32600
    );
}

#[test]
fn rejects_arguments_not_in_the_advertised_tool_schema() {
    let mut server = server();
    initialize(&mut server);
    let response=server.handle_blocking(json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"project_status","arguments":{"key":"TEST-A"}}})).expect("response");
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        response["result"]["structuredContent"]["code"],
        "invalid_request"
    );
}

fn call(server: &mut McpServer, name: &str, arguments: Value) -> Value {
    let request = json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":name,"arguments":arguments}});
    server.handle_blocking(request).expect("response")["result"].clone()
}

#[test]
fn validate_plan_reads_no_workspace_and_matches_the_shared_validation() {
    let mut server = server();
    initialize(&mut server);
    let plan: Value = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("plan");
    let result = call(&mut server, "validate_plan", json!({"plan": plan.clone()}));
    assert_eq!(result["isError"], false);
    let expected = dpm_app::validate_plan(plan.clone()).expect("valid");
    assert_eq!(
        result["structuredContent"],
        json!({"api_version": dpm_app::API_VERSION, "revision": null, "lineage_id": null, "data": expected})
    );
    let mut broken = plan;
    broken["format_version"] = json!(2);
    let refused = call(&mut server, "validate_plan", json!({"plan": broken}));
    assert_eq!(refused["isError"], true);
    assert_eq!(refused["structuredContent"]["code"], "invalid_request");
}

#[test]
fn successes_share_the_cli_envelope() {
    let mut server = server();
    initialize(&mut server);
    let status = call(
        &mut server,
        "project_status",
        json!({"probabilistic": false}),
    );
    let content = &status["structuredContent"];
    assert_eq!(content["api_version"], dpm_app::API_VERSION);
    assert_eq!(content["revision"], 0);
    assert!(content["data"].is_object());
    let claimed = call(
        &mut server,
        "claim_work",
        json!({"key":"TEST-A","base_revision":0}),
    );
    assert_eq!(claimed["structuredContent"]["revision"], 1);
    assert_eq!(
        claimed["structuredContent"]["data"]["resulting_revision"],
        1
    );
}

/// Creating or replacing a workspace is CLI administration by a human or service: every tool
/// that writes either changes an existing workspace at an observed revision or binds an
/// existing store, so no tool call can bootstrap one.
#[test]
fn no_tool_bootstraps_a_workspace() {
    for tool in crate::tools::definitions() {
        let name = tool["name"].as_str().expect("name");
        let read_only = tool["annotations"]["readOnlyHint"] == true;
        let required = tool["inputSchema"]["required"]
            .as_array()
            .expect("required");
        assert!(
            read_only || name == "workspace_register" || required.contains(&json!("base_revision")),
            "{name} writes without an observed revision"
        );
        assert!(
            !matches!(
                name,
                "init" | "import" | "demo" | "import_plan" | "init_workspace"
            ),
            "{name} would bootstrap a workspace"
        );
    }
}
