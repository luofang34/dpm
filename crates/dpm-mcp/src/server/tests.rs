#![allow(clippy::expect_used)]
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
    assert_eq!(list["result"]["tools"].as_array().expect("tools").len(), 13);
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
