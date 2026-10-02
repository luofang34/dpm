#![cfg(test)]
//! The adapter end to end through its public entry: a scripted provider process, a real
//! disposable store, and read-back through the application's own queries.

#[path = "adapter/support.rs"]
mod support;

use dpm_claude::{CommandError, Evidence, execute_blocking};
use dpm_model::{ActivityKind, Observation, ObservedStatus, RunState, WorkStatus};
use serde_json::{Value, json};
use std::{fs, sync::atomic::AtomicBool};
use support::{Fixture, HANDSHAKE, emit, finished, init_event, pilot, text, tool_result, tool_use};

fn never() -> AtomicBool {
    AtomicBool::new(false)
}

fn ask() -> Value {
    json!({"questions": [{"question": "Continue?", "options": ["yes", "no"]}]})
}

fn control(request: &str, tool: &str, tool_use: &str, input: &Value) -> Value {
    json!({"type": "control_request", "request_id": request, "request": {"subtype": "can_use_tool", "tool_name": tool, "tool_use_id": tool_use, "input": input}})
}

/// A provider that reads a file, asks a question that is refused, and finishes its turn.
fn real_turn_script(fixture: &Fixture) -> std::path::PathBuf {
    fixture.script(
        "provider.sh",
        &[
            HANDSHAKE.to_string(),
            emit(&init_event()),
            emit(&tool_use(
                "e1",
                "toolu_r",
                "Read",
                &json!({"file_path": "note.txt"}),
            )),
            emit(&tool_result(
                "toolu_r",
                false,
                "The maintenance fixture says: private body",
            )),
            emit(&tool_use("e2", "toolu_q", "AskUserQuestion", &ask())),
            emit(&control("req-1", "AskUserQuestion", "toolu_q", &ask())),
            "read denial_reply && printf '%s\\n' \"$denial_reply\" > reply.txt\n".to_string(),
            emit(&tool_result("toolu_q", true, "refused by DPM")),
            emit(&text(
                "e3",
                "I read note.txt; the question was refused, so I am stopping.",
            )),
            emit(&finished(
                &json!([{"tool_name": "AskUserQuestion", "tool_use_id": "toolu_q", "tool_input": ask()}]),
            )),
        ]
        .concat(),
    )
}

/// The run as recorded: managed, in its session and turn, with provenance that attests only what
/// was known when it started.
fn check_run(fixture: &Fixture, run: dpm_model::RunId) {
    let view = fixture.app().run_blocking(run).expect("run view").data;
    assert_eq!(
        (view.run.observation, view.state),
        (Observation::Managed, RunState::Completed)
    );
    let session = view.run.session.expect("a provider session");
    assert_eq!(
        (session.provider.as_str(), session.turn.as_deref()),
        ("claude", Some("1"))
    );
    let provenance = session.provenance.expect("provenance");
    assert_eq!(
        provenance.requested_model.as_deref(),
        Some("claude-sonnet-5-5")
    );
    assert_eq!(
        provenance.observed_model, None,
        "this runtime announces itself only after the prompt, so none is attested"
    );
    assert!(
        provenance
            .configuration_digest
            .is_some_and(|digest| digest.len() == 64)
    );
}

/// The activity: every public event in order, contiguous, with no output of a successful tool.
fn check_activity(fixture: &Fixture, run: dpm_model::RunId) {
    let entries = fixture.activity(run);
    let kinds: Vec<ActivityKind> = entries.iter().map(|entry| entry.record.kind).collect();
    assert_eq!(
        kinds,
        [
            ActivityKind::Progress,
            ActivityKind::ToolStarted,
            ActivityKind::ToolResult,
            ActivityKind::ToolStarted,
            ActivityKind::InputRequested,
            ActivityKind::Progress,
            ActivityKind::ToolResult,
            ActivityKind::Progress,
        ],
        "{:#?}",
        fixture.texts(run)
    );
    let sequences: Vec<u64> = entries
        .iter()
        .map(|entry| entry.record.source_sequence)
        .collect();
    assert_eq!(
        sequences,
        (1..=8).collect::<Vec<u64>>(),
        "contiguous and in the order accepted"
    );
    let texts = fixture.texts(run);
    assert!(
        texts[1].contains("Read: ") && texts[2].ends_with("ok, 42 bytes"),
        "{texts:?}"
    );
    assert!(
        texts[5].contains("the refusal was written to the provider"),
        "the fate is recorded after the reply: {texts:?}"
    );
    assert!(
        !texts.join(" ").contains("private body"),
        "a successful tool's output is not retained"
    );
}

/// What the provider saw and what the lifecycle says.
fn check_wire_and_lifecycle(fixture: &Fixture, run: dpm_model::RunId) {
    // The one input request was refused, never approved, on the wire the provider reads.
    let reply: Value =
        serde_json::from_str(&fs::read_to_string(fixture.marker("reply.txt")).expect("reply"))
            .expect("json");
    assert_eq!(reply["response"]["request_id"], "req-1");
    assert_eq!(reply["response"]["response"]["behavior"], "deny");
    // The prompt reached the provider only after the run was recorded.
    assert!(fixture.marker("prompt-seen").exists());
    let lifecycle = fixture.lifecycle(run);
    assert_eq!(
        lifecycle
            .iter()
            .map(|entry| entry.event.state)
            .collect::<Vec<_>>(),
        [RunState::Working, RunState::Completed]
    );
    assert!(
        lifecycle[1]
            .event
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("neither submitted nor verified"))
    );
}

#[test]
fn a_real_turn_is_recorded_as_a_managed_run_with_its_public_events_and_the_task_untouched() {
    let fixture = Fixture::new();
    let before = fixture.project_state();
    let script = real_turn_script(&fixture);
    let outcome = execute_blocking(
        &fixture.options(&script, &["--model", "claude-sonnet-5-5"]),
        &never(),
    )
    .expect("run");
    let report = &outcome.report;
    assert_eq!(
        (report.terminal.state, report.terminal.evidence),
        (RunState::Completed, Evidence::ResultSuccess)
    );
    assert!(report.terminal_recorded && report.sink_failure.is_none());
    check_run(&fixture, outcome.run);
    check_activity(&fixture, outcome.run);
    check_wire_and_lifecycle(&fixture, outcome.run);
    assert_eq!(
        fixture.project_state(),
        before,
        "a run never changes the project's revision or history"
    );
    let task = fixture
        .app()
        .show_blocking("TEST-A")
        .expect("task")
        .data
        .work;
    assert_eq!(
        (task.execution.status, task.execution.owner),
        (WorkStatus::InProgress, Some(pilot())),
        "completion of a turn is not submission or ownership release"
    );
}

#[test]
fn an_unreadable_prompt_file_keeps_its_io_failure_as_the_source() {
    use std::error::Error;
    let fixture = Fixture::new();
    let script = fixture.script("provider.sh", HANDSHAKE);
    let missing = fixture.directory.path().join("absent.txt");
    let refused = execute_blocking(
        &fixture.options(&script, &["--prompt-file", &missing.to_string_lossy()]),
        &never(),
    )
    .expect_err("no prompt");
    assert!(
        matches!(&refused, CommandError::PromptFile { path, .. } if *path == missing),
        "{refused:?}"
    );
    assert!(
        refused
            .source()
            .is_some_and(|source| source.is::<std::io::Error>())
    );
}

#[test]
fn a_task_its_executor_does_not_own_is_refused_before_any_provider_exists() {
    let fixture = Fixture::new();
    let script = fixture.script("provider.sh", HANDSHAKE);
    let mut options = fixture.options(&script, &[]);
    options.executor = Some(dpm_model::ActorId::agent("someone-else"));
    let refused = execute_blocking(&options, &never());
    assert!(
        matches!(refused, Err(CommandError::Task { .. })),
        "{refused:?}"
    );
    assert!(
        !fixture.marker("args.txt").exists(),
        "no provider was started"
    );
}

#[test]
fn each_way_a_turn_can_end_without_a_result_is_its_own_evidence_and_never_completion() {
    let cases: [(&str, &str, RunState, Evidence); 3] = [
        (
            "eof",
            "exit 0\n",
            RunState::Failed,
            Evidence::EofWithoutResult,
        ),
        (
            "nonzero",
            "exit 7\n",
            RunState::Failed,
            Evidence::NonzeroExit,
        ),
        (
            "signal",
            "kill -9 $$\n",
            RunState::Interrupted,
            Evidence::Signal,
        ),
    ];
    for (name, ending, state, evidence) in cases {
        let fixture = Fixture::new();
        let script = fixture.script(
            "provider.sh",
            &[
                HANDSHAKE.to_string(),
                emit(&init_event()),
                emit(&text("e1", "working")),
                ending.to_string(),
            ]
            .concat(),
        );
        let outcome = execute_blocking(&fixture.options(&script, &[]), &never()).expect("run");
        assert_eq!(
            (
                outcome.report.terminal.state,
                outcome.report.terminal.evidence
            ),
            (state, evidence),
            "{name}"
        );
        let view = fixture.app().run_blocking(outcome.run).expect("view").data;
        assert_eq!(view.state, state, "{name}: the recorded lifecycle agrees");
        assert_ne!(view.state, RunState::Completed);
    }
}

#[test]
fn a_result_whose_reason_contradicts_success_is_never_recorded_as_completed() {
    for (name, reason, state, evidence) in [
        (
            "interrupted",
            Some("interrupted"),
            RunState::Interrupted,
            Evidence::ResultInterrupted,
        ),
        (
            "unknown",
            Some("a_reason_nobody_defined"),
            RunState::Failed,
            Evidence::EofWithoutResult,
        ),
        ("absent", None, RunState::Failed, Evidence::EofWithoutResult),
    ] {
        let fixture = Fixture::new();
        let mut result = json!({"type": "result", "subtype": "success", "is_error": false, "session_id": "@SESSION@", "num_turns": 1});
        if let Some(reason) = reason {
            result["terminal_reason"] = json!(reason);
        }
        let script = fixture.script(
            "provider.sh",
            &[HANDSHAKE.to_string(), emit(&init_event()), emit(&result)].concat(),
        );
        let outcome = execute_blocking(&fixture.options(&script, &[]), &never()).expect("run");
        assert_eq!(
            (
                outcome.report.terminal.state,
                outcome.report.terminal.evidence
            ),
            (state, evidence),
            "{name}"
        );
        let view = fixture.app().run_blocking(outcome.run).expect("view").data;
        assert_eq!(view.state, state, "{name}: the store agrees");
        assert!(
            !view
                .state_detail
                .unwrap_or_default()
                .contains("finished turn"),
            "{name}"
        );
    }
}

#[test]
fn unreadable_lines_are_noted_and_the_stream_goes_on() {
    let fixture = Fixture::new();
    let script = fixture.script(
        "provider.sh",
        &[
            HANDSHAKE.to_string(),
            emit(&init_event()),
            "printf 'this is not json\\n'\nhead -c 3000 /dev/zero | tr '\\0' x; echo\nprintf '\\377\\376 bad\\n'\n".to_string(),
            emit(&text("e1", "still here")),
            emit(&finished(&json!([]))),
        ]
        .concat(),
    );
    let outcome = execute_blocking(
        &fixture.options(&script, &["--max-line-bytes", "1024"]),
        &never(),
    )
    .expect("run");
    assert_eq!(outcome.report.terminal.state, RunState::Completed);
    assert_eq!(outcome.report.tally.malformed, 3);
    let texts = fixture.texts(outcome.run).join("\n");
    assert!(
        texts.contains("an oversized line (3000 bytes)")
            && texts.contains("not UTF-8")
            && texts.contains("still here"),
        "{texts}"
    );
}

/// A provider that repeats itself, contradicts itself, and carries material that must not be kept.
fn noisy_script(fixture: &Fixture) -> std::path::PathBuf {
    let secret_text = text(
        "e5",
        "password=hunter2 sk-abcdef1234567890XYZ /Users/someone/work",
    );
    let thinking = json!({"type": "assistant", "uuid": "e6", "session_id": "@SESSION@", "message": {"content": [
        {"type": "thinking", "thinking": "PRIVATE-REASONING", "signature": "SIGNATURE-BYTES"}]}});
    fixture.script(
        "provider.sh",
        &[
            HANDSHAKE.to_string(),
            emit(&init_event()),
            emit(&text("e1", "once")),
            emit(&text("e1", "once")),
            emit(&tool_use(
                "e2",
                "toolu_x",
                "Read",
                &json!({"file_path": "a.txt"}),
            )),
            emit(&tool_use(
                "e3",
                "toolu_x",
                "Read",
                &json!({"file_path": "different.txt"}),
            )),
            emit(&tool_use(
                "e4",
                "toolu_w",
                "Write",
                &json!({"file_path": "out.txt", "content": "FILE-BODY-SECRET"}),
            )),
            emit(&secret_text),
            emit(&thinking),
            emit(&tool_result("toolu_w", true, "TOOL-ERROR-EXCERPT")),
            emit(&finished(&json!([]))),
        ]
        .concat(),
    )
}

#[test]
fn duplicates_conflicts_and_private_material_are_handled_at_the_intake_the_store_sees() {
    let fixture = Fixture::new();
    let script = noisy_script(&fixture);
    let outcome = execute_blocking(&fixture.options(&script, &[]), &never()).expect("run");
    assert_eq!(
        (
            outcome.report.tally.repeated,
            outcome.report.tally.conflicts
        ),
        (1, 1)
    );
    let texts = fixture.texts(outcome.run);
    assert_eq!(
        texts.iter().filter(|text| text.ends_with("once")).count(),
        1,
        "the repeat recorded nothing"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.contains("conflict") && text.contains("tool_use:toolu_x"))
    );
    assert!(
        !texts.iter().any(|text| text.contains("different.txt")),
        "the conflicting payload was not kept"
    );
    let view = serde_json::to_string(&fixture.app().run_blocking(outcome.run).expect("view").data)
        .expect("json");
    let everything = format!(
        "{} {view} {:?}",
        texts.join(" "),
        fixture.lifecycle(outcome.run)
    );
    for private in [
        "hunter2",
        "sk-abcdef",
        "someone",
        "PRIVATE-REASONING",
        "SIGNATURE-BYTES",
        "FILE-BODY-SECRET",
    ] {
        assert!(!everything.contains(private), "{private} reached the store");
    }
    assert!(
        everything.contains("1 reasoning blocks"),
        "what was dropped is counted, not kept"
    );
}

#[test]
fn a_cli_or_mcp_only_agent_is_reported_only_and_silence_is_stale_never_idle_or_finished() {
    use dpm_app::{QueryClock, RunStartRequest};
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let run = dpm_model::RunId::new();
    app.start_run_blocking(RunStartRequest {
        actor: pilot(),
        work_key: "TEST-A".into(),
        executor: None,
        run_id: Some(run),
        parent: None,
        session: None,
        observation: Observation::ReportedOnly,
        sources: Vec::new(),
        observed_at: None,
        base_lineage: None,
    })
    .expect("a self-reported run");
    app.set_query_clock(QueryClock::Fixed(
        chrono::Utc::now() + chrono::TimeDelta::days(3),
    ));
    let view = app.run_blocking(run).expect("view").data;
    assert_eq!(view.run.observation, Observation::ReportedOnly);
    assert_eq!(
        (view.state, view.status),
        (RunState::Working, ObservedStatus::Stale),
        "no events imply neither idle nor finished"
    );
}
