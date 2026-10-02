//! The shipped helper binary as a host runs it: real process, real pipes, real exit codes.
#![cfg(test)]

use dpm_app::Application;
use dpm_model::Plan;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

fn plan() -> Plan {
    let mut plan: Plan =
        serde_json::from_str(include_str!("../../../tests/support/execution-plan.json"))
            .expect("fixture");
    plan.decisions.clear();
    plan
}

fn helper(arguments: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dpm-native"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the helper starts");
    // A write error means the helper has already answered and left, which the exit code reports.
    child.stdin.take().expect("stdin").write_all(stdin).ok();
    child.wait_with_output().expect("the helper exits")
}

fn frames(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .expect("standard output is UTF-8")
        .lines()
        .map(|line| {
            serde_json::from_str(line).expect("standard output carries protocol frames only")
        })
        .collect()
}

fn line(id: &str, call: &Value) -> Vec<u8> {
    let mut bytes = json!({"protocol": 1, "id": id, "call": call})
        .to_string()
        .into_bytes();
    bytes.push(b'\n');
    bytes
}

fn store(directory: &Path) -> std::path::PathBuf {
    let path = directory.join("state.sqlite");
    drop(Application::initialize_blocking(&path, &plan()).expect("store"));
    path
}

#[test]
fn a_missing_workspace_is_refused_with_a_structured_frame_and_the_start_exit_code() {
    let directory = tempfile::tempdir().expect("directory");
    let missing = directory.path().join("absent.sqlite");
    let output = helper(&["--database", missing.to_str().expect("path")], b"");
    assert_eq!(output.status.code(), Some(2));
    let refusal = &frames(&output)[0];
    assert_eq!(
        (refusal["ok"].as_bool(), refusal["id"].as_str()),
        (Some(false), Some(""))
    );
    assert!(
        refusal["error"]["code"]
            .as_str()
            .is_some_and(|code| !code.is_empty())
    );
    assert!(!missing.exists(), "nothing is initialized");
    // Diagnostics are on standard error, through tracing, not mixed into the protocol channel.
    assert!(String::from_utf8_lossy(&output.stderr).contains("not serving"));
}

#[test]
fn bad_arguments_are_refused_the_same_way() {
    let output = helper(&["--max-response-bytes", "12"], b"");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(frames(&output)[0]["error"]["code"], "invalid_options");
    let help = helper(&["--help"], b"");
    assert_eq!(help.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&help.stdout).starts_with("Usage: dpm-native"));
}

#[test]
fn a_session_serves_in_order_and_a_clean_close_exits_zero() {
    let directory = tempfile::tempdir().expect("directory");
    let path = store(directory.path());
    let mut input = line("h", &json!({"type": "hello", "protocols": [1]}));
    input.extend(line("a", &json!({"type": "attach"})));
    input.extend(line(
        "s",
        &json!({"type": "query", "query": {"query": "status", "probabilistic": false}}),
    ));
    let output = helper(&["--database", path.to_str().expect("path")], &input);
    assert_eq!(output.status.code(), Some(0));
    let answers = frames(&output);
    let ids: Vec<_> = answers.iter().map(|a| a["id"].as_str()).collect();
    assert_eq!(ids, [Some("h"), Some("a"), Some("s")]);
    assert!(answers.iter().all(|answer| answer["ok"] == true));
}

#[test]
fn input_that_ends_inside_a_frame_exits_with_the_truncated_code_after_saying_so() {
    let directory = tempfile::tempdir().expect("directory");
    let path = store(directory.path());
    let mut input = line("a", &json!({"type": "attach"}));
    input.extend(b"{\"protocol\":1,\"id\":\"cut\"");
    let output = helper(&["--database", path.to_str().expect("path")], &input);
    assert_eq!(output.status.code(), Some(3));
    let answers = frames(&output);
    assert_eq!(answers[0]["id"], "a");
    assert_eq!(answers[1]["error"]["code"], "truncated_frame");
}

#[test]
fn frames_obey_their_bounds_in_both_directions_through_the_real_process() {
    let directory = tempfile::tempdir().expect("directory");
    let path = store(directory.path());
    let database = path.to_str().expect("path");

    // A request over the bound is refused unread, and the next request is still answered.
    let mut input = vec![b'x'; 300_000];
    input.push(b'\n');
    input.extend(line("after", &json!({"type": "attach"})));
    let output = helper(
        &["--database", database, "--max-request-bytes", "2048"],
        &input,
    );
    assert_eq!(output.status.code(), Some(0));
    let answers = frames(&output);
    assert_eq!(answers[0]["error"]["code"], "frame_too_large");
    assert_eq!(answers[1]["id"], "after");

    // An answer over the bound is replaced whole, and every line stays within it.
    let mut input = line(
        "export",
        &json!({"type": "query", "query": {"query": "export"}}),
    );
    // An identifier longer than is accepted is refused, not echoed, and never processed.
    input.extend(line(
        &"i".repeat(10_000),
        &json!({"type": "hello", "protocols": [1]}),
    ));
    let output = helper(
        &["--database", database, "--max-response-bytes", "4096"],
        &input,
    );
    assert_eq!(output.status.code(), Some(0));
    for raw in output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|part| !part.is_empty())
    {
        assert!(
            raw.len() <= 4096,
            "a frame of {} bytes broke the 4096-byte bound",
            raw.len()
        );
    }
    let answers = frames(&output);
    assert_eq!(answers[0]["id"], "export");
    assert_eq!(answers[0]["error"]["code"], "response_too_large");
    assert!(
        answers[0].get("result").is_none(),
        "no part of the answer is sent"
    );
    assert_eq!(
        (
            answers[1]["id"].as_str(),
            answers[1]["error"]["code"].as_str()
        ),
        (Some(""), Some("invalid_id"))
    );
}

#[test]
fn a_command_in_a_session_the_client_abandons_is_still_committed() {
    let directory = tempfile::tempdir().expect("directory");
    let path = store(directory.path());
    let work = plan().find_work_by_key("TEST-A").expect("TEST-A").id;
    let command = line(
        "claim",
        &json!({"type": "command", "request": {
            "actor": {"kind": "Agent", "name": "worker"},
            "base_revision": 0,
            "command": {"Claim": {"work": work}},
        }}),
    );
    // The client sends the command and closes its pipes without reading the answer.
    let mut child = Command::new(env!("CARGO_BIN_EXE_dpm-native"))
        .args(["--database", path.to_str().expect("path")])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the helper starts");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&command)
        .expect("send");
    let status = child.wait().expect("the helper exits");
    // The answer could not be delivered, which is the helper's failure exit; the command is not undone.
    assert!(matches!(status.code(), Some(0 | 1)), "{status:?}");
    let history = Application::open_blocking(&path)
        .expect("reopen")
        .history_blocking(0, 10)
        .expect("history");
    assert_eq!(history.entries.len(), 1);
}

#[test]
fn a_startup_refusal_obeys_the_response_bound_even_when_it_would_echo_a_long_path() {
    let directory = tempfile::tempdir().expect("directory");
    let long = format!("{}/{}", directory.path().display(), "x".repeat(10_000));
    let output = helper(&["--database", &long, "--max-response-bytes", "1024"], b"");
    assert_eq!(output.status.code(), Some(2));
    for raw in output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|part| !part.is_empty())
    {
        assert!(
            raw.len() <= 1024,
            "a startup frame of {} bytes broke the bound",
            raw.len()
        );
    }
    let refusal = &frames(&output)[0];
    assert_eq!(refusal["ok"], false);
    assert!(
        refusal["error"]["code"]
            .as_str()
            .is_some_and(|code| !code.is_empty()),
        "the code survives"
    );
    // Diagnostics keep the full text; only the frame is bounded.
    assert!(String::from_utf8_lossy(&output.stderr).len() > 10_000);
}

#[test]
fn a_committed_command_at_the_smallest_bound_keeps_correlation_and_identity_through_the_process() {
    let directory = tempfile::tempdir().expect("directory");
    let path = store(directory.path());
    let work = plan().find_work_by_key("TEST-A").expect("TEST-A").id;
    let id = "C".repeat(128);
    let step = |id: &str, base: u64, command: Value| {
        line(
            id,
            &json!({"type": "command", "request": {
                "actor": {"kind": "Agent", "name": "worker"},
                "base_revision": base,
                "command": command,
            }}),
        )
    };
    let mut input = step("c1", 0, json!({"Claim": {"work": work}}));
    input.extend(step("c2", 1, json!({"Start": {"work": work}})));
    input.extend(step(
        &id,
        2,
        json!({"ReportProgress": {"work": work, "percent": 5, "note": "n".repeat(3000)}}),
    ));
    let output = helper(
        &[
            "--database",
            path.to_str().expect("path"),
            "--max-response-bytes",
            "1024",
        ],
        &input,
    );
    assert_eq!(output.status.code(), Some(0));
    for raw in output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|part| !part.is_empty())
    {
        assert!(raw.len() <= 1024, "{} bytes", raw.len());
    }
    let answers = frames(&output);
    assert_eq!(answers[2]["id"], id.as_str());
    assert_eq!(answers[2]["error"]["code"], "response_too_large");
    assert_eq!(
        answers[2]["error"]["details"]["committed"]["resulting_revision"],
        3
    );
    let history = Application::open_blocking(&path)
        .expect("reopen")
        .history_blocking(0, 10)
        .expect("history");
    assert_eq!(history.entries.len(), 3, "the mutation committed");
    assert_eq!(
        answers[2]["error"]["details"]["committed"]["operation_id"],
        json!(history.entries[2].operation.operation.id)
    );
}
