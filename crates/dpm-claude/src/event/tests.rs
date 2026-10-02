use super::*;
use serde_json::json;

fn parse(value: &Value) -> Parsed {
    parse_line(&value.to_string()).expect("a provider event")
}

#[test]
fn init_keeps_only_configuration_labels_and_never_the_account_or_listings() {
    let parsed = parse(&json!({
        "type": "system", "subtype": "init", "session_id": "b11e7a47-6e43-4f6e-b1c5-10216aaba204",
        "model": "claude-sonnet-5-5", "permissionMode": "default", "claude_code_version": "2.1.287",
        "apiKeySource": "none", "tools": ["Read", "Write"],
        "account": {"email": "someone@example.com", "token": "sk-abcdef1234567890XYZ"},
        "slash_commands": ["a", "b"], "mcp_servers": [{"env": {"TOKEN": "hunter2"}}],
        "messaging_socket_path": "/Users/someone/private.sock"
    }));
    let [PublicEvent::Initialized(session)] = parsed.events.as_slice() else {
        panic!("one session event: {parsed:?}");
    };
    assert_eq!(session.id, "b11e7a47-6e43-4f6e-b1c5-10216aaba204");
    assert_eq!(session.model.as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(session.key_source.as_deref(), Some("none"));
    assert_eq!(session.tool_count, 2);
    let shown = format!("{parsed:?}");
    for private in ["someone", "sk-abcdef", "hunter2", "private.sock", "slash"] {
        assert!(!shown.contains(private), "{private} was copied out of init");
    }
}

#[test]
fn thinking_and_redacted_thinking_are_counted_and_never_copied() {
    let parsed = parse(&json!({
        "type": "assistant", "uuid": "u1", "message": {"id": "m1", "content": [
            {"type": "thinking", "thinking": "private chain of thought", "signature": "SIGNATUREDATA"},
            {"type": "redacted_thinking", "data": "OPAQUEDATA"},
            {"type": "text", "text": "Public answer."},
            {"type": "image", "source": "ignored"}
        ]}
    }));
    assert_eq!(parsed.dropped.hidden_reasoning, 2);
    assert_eq!(parsed.dropped.unlisted_blocks, 1);
    assert_eq!(
        parsed.events,
        vec![PublicEvent::Text {
            id: "u1.2".into(),
            text: "Public answer.".into()
        }]
    );
    let shown = format!("{parsed:?}");
    for private in ["chain of thought", "SIGNATUREDATA", "OPAQUEDATA"] {
        assert!(!shown.contains(private), "{private}");
    }
}

#[test]
fn tool_calls_keep_one_identifying_value_and_never_the_arguments() {
    let parsed = parse(&json!({
        "type": "assistant", "uuid": "u2", "message": {"id": "m2", "content": [
            {"type": "tool_use", "id": "toolu_1", "name": "Write",
             "input": {"file_path": "/Users/someone/work/out.txt", "content": "TOPSECRETBODY token=hunter2"}},
            {"type": "tool_use", "id": "toolu_2", "name": "Bash", "input": {"command": "curl -H 'Authorization: Bearer abcdefghijkl' x"}},
            {"type": "tool_use", "id": "toolu_3", "name": "UnknownTool", "input": {"anything": "private"}},
            {"type": "tool_use", "id": "toolu_4", "name": "AskUserQuestion",
             "input": {"questions": [{"question": "Continue?", "options": [{"label": "yes"}, {"label": "no"}]}]}}
        ]}
    }));
    let details: Vec<(&str, &str, &str)> = parsed
        .events
        .iter()
        .map(|event| match event {
            PublicEvent::ToolUse { id, tool, detail } => {
                (id.as_str(), tool.as_str(), detail.as_str())
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(details[0], ("toolu_1", "Write", "~/work/out.txt"));
    assert!(details[1].2.contains("<redacted>") && !details[1].2.contains("abcdefghijkl"));
    assert_eq!(details[2], ("toolu_3", "UnknownTool", ""));
    assert_eq!(
        details[3],
        ("toolu_4", "AskUserQuestion", "Continue? [yes/no]")
    );
    assert!(!format!("{parsed:?}").contains("TOPSECRETBODY"));
}

#[test]
fn a_fetched_url_keeps_scheme_host_and_path_and_never_credentials_query_or_fragment() {
    let detail = |url: &str| tool_detail("WebFetch", &json!({"url": url}));
    assert_eq!(
        detail("https://user:synthetic-pass@example.invalid/a/b?token=abc#frag"),
        "https://example.invalid/a/b"
    );
    assert_eq!(detail("https://example.invalid"), "https://example.invalid");
    assert_eq!(
        detail("http://a@b@example.invalid/x"),
        "http://example.invalid/x"
    );
    assert_eq!(detail("example.invalid/path?q=1"), "example.invalid/path");
}

#[test]
fn two_tool_calls_of_one_message_keep_their_own_identities() {
    let first = parse(
        &json!({"type": "assistant", "uuid": "event-a", "message": {"id": "msg_same", "content": [
        {"type": "tool_use", "id": "toolu_a", "name": "Read", "input": {"file_path": "a.txt"}}]}}),
    );
    let second = parse(
        &json!({"type": "assistant", "uuid": "event-b", "message": {"id": "msg_same", "content": [
        {"type": "tool_use", "id": "toolu_b", "name": "Read", "input": {"file_path": "b.txt"}}]}}),
    );
    assert_ne!(first.events, second.events);
}

#[test]
fn a_successful_result_keeps_its_size_and_a_failed_one_a_bounded_excerpt() {
    let parsed = parse(&json!({"type": "user", "message": {"content": [
        {"type": "tool_result", "tool_use_id": "toolu_1", "content": "The maintenance fixture says: private body"},
        {"type": "tool_result", "tool_use_id": "toolu_2", "is_error": true, "content": [{"type": "text", "text": "refused: no approvals"}]}
    ]}}));
    assert_eq!(
        parsed.events,
        vec![
            PublicEvent::ToolResult {
                id: "toolu_1".into(),
                error: false,
                detail: "ok, 42 bytes".into()
            },
            PublicEvent::ToolResult {
                id: "toolu_2".into(),
                error: true,
                detail: "refused: no approvals".into()
            },
        ]
    );
}

#[test]
fn a_permission_request_names_its_request_and_tool_use_and_keeps_no_arguments() {
    let parsed = parse(
        &json!({"type": "control_request", "request_id": "29603084-0bfd-4f35-a482-b83d745f3bec", "request": {
        "subtype": "can_use_tool", "tool_name": "Write", "tool_use_id": "toolu_w",
        "input": {"file_path": "out.txt", "content": "TOPSECRETBODY"}, "description": "out.txt",
        "permission_suggestions": [{"type": "addRules"}]}}),
    );
    assert_eq!(
        parsed.events,
        vec![PublicEvent::InputRequest {
            request: "29603084-0bfd-4f35-a482-b83d745f3bec".into(),
            tool_use: Some("toolu_w".into()),
            tool: "Write".into(),
            detail: "out.txt".into(),
        }]
    );
}

#[test]
fn control_requests_of_other_kinds_are_reported_unsupported_and_answers_are_read() {
    let hook = parse(
        &json!({"type": "control_request", "request_id": "r9", "request": {"subtype": "hook_callback"}}),
    );
    assert_eq!(
        hook.events,
        vec![PublicEvent::UnsupportedControl {
            request: "r9".into(),
            subtype: "hook_callback".into()
        }]
    );
    let answer = parse(
        &json!({"type": "control_response", "response": {"subtype": "success", "request_id": "req_1", "response": {"account": {"email": "x"}}}}),
    );
    assert_eq!(
        answer.events,
        vec![PublicEvent::Answered {
            request: "req_1".into(),
            ok: true
        }]
    );
}

#[test]
fn the_terminal_result_distinguishes_success_from_flagged_and_recaps_denials() {
    let parsed = parse(
        &json!({"type": "result", "subtype": "success", "is_error": false, "num_turns": 4,
        "terminal_reason": "completed", "result": "Public summary", "total_cost_usd": 0.03,
        "permission_denials": [{"tool_name": "Write", "tool_use_id": "toolu_w", "tool_input": {"file_path": "out.txt", "content": "x"}}]}),
    );
    assert_eq!(
        parsed.events,
        vec![
            PublicEvent::Denied {
                tool_use: Some("toolu_w".into()),
                tool: "Write".into(),
                detail: "out.txt".into()
            },
            PublicEvent::Finished(Finish {
                verdict: Verdict::Completed,
                subtype: "success".into(),
                terminal_reason: Some("completed".into()),
                turns: Some(4),
                error: None,
            }),
        ]
    );
    // A provider failure can arrive under the success subtype with the error flag set.
    let flagged = parse(
        &json!({"type": "result", "subtype": "success", "is_error": true, "terminal_reason": "completed", "result": "API error: overloaded"}),
    );
    let [PublicEvent::Finished(finish)] = flagged.events.as_slice() else {
        panic!("{flagged:?}")
    };
    assert_eq!(finish.verdict, Verdict::Failed);
    assert_eq!(finish.error.as_deref(), Some("API error: overloaded"));
}

#[test]
fn events_outside_the_allowlist_are_counted_and_dropped() {
    for kind in [
        json!({"type": "rate_limit_event", "rate_limit_info": {"a": 1}}),
        json!({"type": "system", "subtype": "thinking_tokens", "estimated_tokens": 5}),
        json!({"type": "stream_event", "event": {"delta": {"text": "partial"}}}),
    ] {
        let parsed = parse(&kind);
        assert!(parsed.events.is_empty(), "{kind}");
        assert_eq!(parsed.dropped.unlisted_events, 1);
    }
}

#[test]
fn lines_that_are_not_events_are_refused_with_a_bounded_reason() {
    assert!(matches!(
        parse_line("not json at all"),
        Err(ParseError::NotJson(_))
    ));
    assert_eq!(parse_line("[1,2]"), Err(ParseError::NotAnEvent));
    assert_eq!(parse_line("\"text\""), Err(ParseError::NotAnEvent));
}

#[test]
fn identifiers_that_are_not_identifier_shaped_are_not_kept() {
    let parsed = parse(
        &json!({"type": "assistant", "uuid": "u3", "message": {"content": [
        {"type": "tool_use", "id": "has space and /etc/passwd", "name": "Read", "input": {"file_path": "a"}}]}}),
    );
    assert!(parsed.events.is_empty());
    assert_eq!(parsed.dropped.unidentified, 1);
}

#[test]
fn success_needs_the_subtype_the_error_flag_and_the_providers_own_reason_to_agree() {
    let result = |subtype: &str, flagged: bool, reason: Option<&str>| {
        let mut value = json!({"type": "result", "subtype": subtype, "is_error": flagged});
        if let Some(reason) = reason {
            value["terminal_reason"] = json!(reason);
        }
        parse(&value).events
    };
    let verdict = |events: &[PublicEvent]| match events {
        [PublicEvent::Finished(finish)] => Some(finish.verdict),
        _ => None,
    };
    assert_eq!(
        verdict(&result("success", false, Some("completed"))),
        Some(Verdict::Completed)
    );
    // An explicit interruption is an interruption, whatever the subtype and flag say.
    for reason in [
        "interrupted",
        "cancelled",
        "user_cancelled",
        "aborted_streaming",
    ] {
        assert_eq!(
            verdict(&result("success", false, Some(reason))),
            Some(Verdict::Interrupted),
            "{reason}"
        );
        assert_eq!(
            verdict(&result("error_during_execution", true, Some(reason))),
            Some(Verdict::Interrupted),
            "{reason}"
        );
    }
    // Errors fail, whatever the reason.
    assert_eq!(
        verdict(&result("error_max_turns", false, Some("completed"))),
        Some(Verdict::Failed)
    );
    assert_eq!(
        verdict(&result("success", true, Some("completed"))),
        Some(Verdict::Failed)
    );
    // A success with no reason, or one that is not recognized, ends nothing at all.
    for reason in [None, Some("max_turns"), Some("something_new")] {
        let events = result("success", false, reason);
        assert!(
            matches!(events.as_slice(), [PublicEvent::MalformedResult { .. }]),
            "{reason:?}: {events:?}"
        );
    }
}
