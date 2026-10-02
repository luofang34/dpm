use super::*;
use dpm_model::RunState;
use serde_json::{Value, json};

const SESSION: &str = "11111111-2222-4333-8444-555555555555";

fn intake() -> Intake {
    Intake::new(RunId::new(), 0, Some(SESSION.into()))
}

fn line(value: &Value) -> Frame {
    Frame::Line(value.to_string())
}

fn init() -> Frame {
    line(
        &json!({"type": "system", "subtype": "init", "session_id": SESSION, "model": "claude-sonnet-5-5",
        "permissionMode": "default", "claude_code_version": "2.1.287", "apiKeySource": "none", "tools": ["Read"]}),
    )
}

fn started(intake: &mut Intake) {
    let step = intake.accept(init());
    assert_eq!(step.records.len(), 1);
}

fn tool_use(uuid: &str, id: &str, path: &str) -> Frame {
    line(
        &json!({"type": "assistant", "uuid": uuid, "session_id": SESSION, "message": {"id": "msg_x", "content": [
        {"type": "tool_use", "id": id, "name": "Read", "input": {"file_path": path}}]}}),
    )
}

fn texts(step: &Step) -> Vec<String> {
    step.records
        .iter()
        .filter_map(|record| record.text.clone())
        .collect()
}

fn result(subtype: &str, flagged: bool) -> Frame {
    line(
        &json!({"type": "result", "subtype": subtype, "is_error": flagged, "session_id": SESSION, "num_turns": 2, "terminal_reason": "completed"}),
    )
}

#[test]
fn records_get_strictly_increasing_sequences_from_the_cursor_and_in_the_order_accepted() {
    let mut intake = Intake::new(RunId::new(), 41, Some(SESSION.into()));
    let mut sequences = Vec::new();
    for frame in [
        init(),
        tool_use("e1", "toolu_1", "a.txt"),
        tool_use("e2", "toolu_2", "b.txt"),
    ] {
        sequences.extend(
            intake
                .accept(frame)
                .records
                .iter()
                .map(|record| record.source_sequence),
        );
    }
    assert_eq!(sequences, [42, 43, 44]);
}

#[test]
fn a_repeated_event_with_the_same_content_records_nothing_new() {
    let mut intake = intake();
    started(&mut intake);
    let first = intake.accept(tool_use("e1", "toolu_1", "a.txt"));
    let again = intake.accept(tool_use("e1", "toolu_1", "a.txt"));
    assert_eq!(first.records.len(), 1);
    assert!(again.records.is_empty());
    assert_eq!(intake.tally().repeated, 1);
}

#[test]
fn a_repeated_identity_with_different_content_is_a_reported_conflict_and_the_first_stands() {
    let mut intake = intake();
    started(&mut intake);
    intake.accept(tool_use("e1", "toolu_1", "a.txt"));
    let step = intake.accept(tool_use("e9", "toolu_1", "other.txt"));
    let notes = texts(&step);
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0].contains("conflict") && notes[0].contains("tool_use:toolu_1"),
        "{notes:?}"
    );
    assert!(
        !notes[0].contains("other.txt"),
        "the conflicting payload is not kept"
    );
    assert_eq!(intake.tally().conflicts, 1);
}

#[test]
fn one_input_request_per_tool_use_however_the_provider_reports_it() {
    let mut intake = intake();
    started(&mut intake);
    let control = |request: &str| {
        line(
            &json!({"type": "control_request", "request_id": request, "request": {
            "subtype": "can_use_tool", "tool_name": "AskUserQuestion", "tool_use_id": "toolu_q",
            "input": {"questions": [{"question": "Continue?", "options": ["yes", "no"]}]}}}),
        )
    };
    let first = intake.accept(control("r1"));
    let repeated = intake.accept(control("r2"));
    let recap = intake.accept(line(
        &json!({"type": "result", "subtype": "success", "is_error": false, "session_id": SESSION,
        "permission_denials": [{"tool_name": "AskUserQuestion", "tool_use_id": "toolu_q",
            "tool_input": {"questions": [{"question": "Continue?", "options": ["yes", "no"]}]}}]}),
    ));
    let requested: Vec<&ActivityInput> = [&first, &repeated, &recap]
        .iter()
        .flat_map(|step| step.records.iter())
        .filter(|record| record.kind == ActivityKind::InputRequested)
        .collect();
    assert_eq!(requested.len(), 1, "{requested:?}");
    assert!(
        requested[0]
            .text
            .as_deref()
            .is_some_and(|text| text.contains("Continue? [yes/no]"))
    );
    // Every live request is still refused, though it records nothing new.
    assert_eq!(
        first.actions,
        [Action::Deny {
            request: "r1".into(),
            subject: "toolu_q".into()
        }]
    );
    assert_eq!(
        repeated.actions,
        [Action::Deny {
            request: "r2".into(),
            subject: "toolu_q".into()
        }]
    );
}

#[test]
fn the_fate_of_a_refusal_is_recorded_only_after_the_reply_was_written_or_failed() {
    let mut intake = intake();
    started(&mut intake);
    let delivered = intake.replied("toolu_q", Ok(()));
    assert!(texts(&delivered)[0].contains("was written to the provider"));
    let failed = intake.replied("toolu_q", Err("broken pipe".into()));
    let text = &texts(&failed)[0];
    assert!(
        text.contains("could not write the refusal") && !text.contains("was written"),
        "{text}"
    );
}

#[test]
fn a_session_must_initialize_first_and_other_sessions_and_subagents_never_end_the_run() {
    let mut intake = intake();
    // Before initialization nothing session-scoped is recorded.
    assert!(
        intake
            .accept(tool_use("e0", "toolu_0", "early.txt"))
            .records
            .is_empty()
    );
    started(&mut intake);
    // Another session's events are ignored.
    let other = intake.accept(line(&json!({"type": "assistant", "uuid": "e5", "session_id": "someone-else", "message": {"content": [
        {"type": "text", "text": "not ours"}]}})));
    assert!(other.records.is_empty());
    // A subagent's own events are not this run's activity, and its result and another session's
    // result end nothing.
    let subagent = intake.accept(line(&json!({"type": "assistant", "uuid": "e6", "session_id": SESSION, "parent_tool_use_id": "toolu_sub", "message": {"content": [
        {"type": "text", "text": "the subagent's words"}]}})));
    assert!(subagent.records.is_empty());
    intake.accept(line(&json!({"type": "result", "subtype": "success", "is_error": false, "terminal_reason": "completed", "session_id": SESSION, "parent_tool_use_id": "toolu_sub"})));
    intake.accept(line(&json!({"type": "result", "subtype": "success", "is_error": false, "terminal_reason": "completed", "session_id": "someone-else"})));
    assert!(intake.finished().is_none());
    // A different session's init is refused and adopted by no one.
    let imposter = intake.accept(line(
        &json!({"type": "system", "subtype": "init", "session_id": "someone-else"}),
    ));
    assert!(texts(&imposter)[0].contains("session-mismatch"));
    assert!(intake.tally().foreign >= 4);
}

#[test]
fn a_result_without_typed_evidence_ends_nothing_and_leaves_the_run_unfinished() {
    let mut intake = intake();
    started(&mut intake);
    for malformed in [
        json!({"type": "result", "subtype": "success", "session_id": SESSION}),
        json!({"type": "result", "subtype": "success", "is_error": "no", "session_id": SESSION}),
        json!({"type": "result", "is_error": false, "session_id": SESSION}),
    ] {
        let step = intake.accept(line(&malformed));
        assert!(texts(&step)[0].contains("malformed-result"), "{malformed}");
    }
    assert!(intake.finished().is_none());
    let closing = intake.finish(&End {
        exit: Some(Exit {
            code: Some(0),
            signal: None,
        }),
        stopped: None,
        unresolved: false,
    });
    assert_eq!(
        (closing.terminal.state, closing.terminal.evidence),
        (RunState::Failed, Evidence::EofWithoutResult)
    );
}

fn gone(exit: Option<Exit>, stopped: Option<Stop>) -> End {
    End {
        exit,
        stopped,
        unresolved: false,
    }
}

fn clean() -> Option<Exit> {
    Some(Exit {
        code: Some(0),
        signal: None,
    })
}

fn with_reason(reason: &str) -> Frame {
    line(
        &json!({"type": "result", "subtype": "success", "is_error": false, "session_id": SESSION, "terminal_reason": reason}),
    )
}

/// A started session, an optional last frame, and the evidence the ending gives.
fn ending(frame: Option<Frame>, end: End, state: RunState, evidence: Evidence) {
    let mut intake = intake();
    started(&mut intake);
    if let Some(frame) = frame {
        intake.accept(frame);
    }
    let closing = intake.finish(&end);
    assert_eq!(
        (closing.terminal.state, closing.terminal.evidence),
        (state, evidence),
        "{end:?}"
    );
}

#[test]
fn only_the_providers_explicit_result_ends_a_run_by_its_verdict() {
    let end = || gone(clean(), None);
    let (done, failed) = (RunState::Completed, RunState::Failed);
    ending(
        Some(result("success", false)),
        end(),
        done,
        Evidence::ResultSuccess,
    );
    ending(
        Some(result("success", true)),
        end(),
        failed,
        Evidence::ResultError,
    );
    ending(
        Some(result("error_max_turns", false)),
        end(),
        failed,
        Evidence::ResultError,
    );
    ending(
        Some(with_reason("interrupted")),
        end(),
        RunState::Interrupted,
        Evidence::ResultInterrupted,
    );
    // A success with an unrecognized reason ends nothing, so the turn is simply unfinished.
    ending(
        Some(with_reason("new_reason")),
        end(),
        failed,
        Evidence::EofWithoutResult,
    );
}

#[test]
fn every_ending_without_a_result_is_its_own_evidence_and_never_completion() {
    let failed = RunState::Failed;
    let interrupted = RunState::Interrupted;
    let code = |code| {
        Some(Exit {
            code: Some(code),
            signal: None,
        })
    };
    let signal = Some(Exit {
        code: None,
        signal: Some(9),
    });
    ending(
        None,
        gone(clean(), None),
        failed,
        Evidence::EofWithoutResult,
    );
    ending(None, gone(code(7), None), failed, Evidence::NonzeroExit);
    ending(None, gone(signal, None), interrupted, Evidence::Signal);
    for stop in [Stop::Deadline, Stop::Cancelled, Stop::TelemetryLoss] {
        ending(
            None,
            gone(None, Some(stop)),
            interrupted,
            Evidence::AdapterStopped,
        );
    }
    ending(
        None,
        gone(clean(), Some(Stop::Protocol("x".into()))),
        failed,
        Evidence::ProtocolFailure,
    );
}

#[test]
fn a_clean_exit_is_never_a_completed_turn_and_the_completion_says_the_task_is_untouched() {
    let mut eof = intake();
    started(&mut eof);
    let closing = eof.finish(&End {
        exit: Some(Exit {
            code: Some(0),
            signal: None,
        }),
        stopped: None,
        unresolved: false,
    });
    assert_ne!(closing.terminal.state, RunState::Completed);
    assert!(closing.terminal.detail.contains("unfinished"));
    let mut done = intake();
    started(&mut done);
    done.accept(result("success", false));
    let closing = done.finish(&End {
        exit: Some(Exit {
            code: Some(0),
            signal: None,
        }),
        stopped: None,
        unresolved: false,
    });
    assert!(
        closing
            .terminal
            .detail
            .contains("neither submitted nor verified")
    );
}

#[test]
fn an_unresolved_process_is_stated_in_the_terminal_detail() {
    let mut intake = intake();
    started(&mut intake);
    let closing = intake.finish(&End {
        exit: None,
        stopped: Some(Stop::Deadline),
        unresolved: true,
    });
    assert!(closing.terminal.detail.contains("could not be reaped"));
}

#[test]
fn malformed_lines_are_noted_one_by_one_up_to_a_bound_then_summarized_once() {
    let mut intake = intake();
    started(&mut intake);
    let mut noted = 0;
    for _ in 0..MALFORMED_NOTES + 5 {
        noted += intake.accept(Frame::Line("not json".into())).records.len();
    }
    noted += intake.accept(Frame::Oversized { bytes: 99 }).records.len();
    noted += intake.accept(Frame::NotUtf8 { bytes: 9 }).records.len();
    assert_eq!(noted, MALFORMED_NOTES as usize);
    let closing = intake.finish(&End {
        exit: Some(Exit::default()),
        stopped: None,
        unresolved: false,
    });
    let summary = closing
        .records
        .iter()
        .filter_map(|record| record.text.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(summary.contains("further malformed lines"), "{summary}");
}

#[test]
fn events_without_a_provider_identity_are_refused_never_given_one() {
    let mut intake = intake();
    started(&mut intake);
    let step = intake.accept(line(&json!({"type": "assistant", "session_id": SESSION, "message": {"content": [{"type": "text", "text": "no event uuid"}]}})));
    assert!(step.records.is_empty());
    assert_eq!(intake.tally().dropped.unidentified, 1);
    let closing = intake.finish(&End::default());
    assert!(
        closing.records[0]
            .text
            .as_deref()
            .is_some_and(|text| text.contains("without a provider identity"))
    );
}

#[test]
fn nothing_private_survives_into_any_record() {
    let mut intake = intake();
    started(&mut intake);
    let mut everything = Vec::new();
    for frame in [
        line(
            &json!({"type": "assistant", "uuid": "p1", "session_id": SESSION, "message": {"content": [
            {"type": "thinking", "thinking": "PRIVATE-REASONING", "signature": "SIGNATURE-BYTES"},
            {"type": "text", "text": "password=hunter2 and sk-abcdef1234567890XYZ in /Users/someone/x"},
            {"type": "tool_use", "id": "toolu_w", "name": "Write", "input": {"file_path": "out.txt", "content": "FILE-BODY-SECRET"}}]}}),
        ),
        line(
            &json!({"type": "user", "session_id": SESSION, "message": {"content": [
            {"type": "tool_result", "tool_use_id": "toolu_w", "content": "TOOL-OUTPUT-SECRET"}]}}),
        ),
        line(
            &json!({"type": "system", "subtype": "init", "session_id": SESSION, "account": {"email": "someone@example.com"}}),
        ),
    ] {
        everything.extend(texts(&intake.accept(frame)));
    }
    everything.extend(
        intake
            .finish(&End::default())
            .records
            .into_iter()
            .filter_map(|record| record.text),
    );
    let all = everything.join("\n");
    for private in [
        "PRIVATE-REASONING",
        "SIGNATURE-BYTES",
        "hunter2",
        "sk-abcdef",
        "someone",
        "FILE-BODY-SECRET",
        "TOOL-OUTPUT-SECRET",
        "email",
    ] {
        assert!(!all.contains(private), "{private} was retained: {all}");
    }
    assert!(all.contains("ok, 18 bytes") && all.contains("Write: out.txt"));
}

#[test]
fn the_replay_window_is_bounded_and_said_to_be_incomplete_once_it_forgets() {
    let mut intake = intake();
    started(&mut intake);
    assert!(intake.window_complete());
    for index in 0..=SEEN_LIMIT {
        intake.accept(tool_use(
            &format!("e{index}"),
            &format!("toolu_{index}"),
            "a.txt",
        ));
    }
    assert!(
        !intake.window_complete(),
        "more identities passed than the window holds"
    );
    // The first identity was forgotten, so its replay is recorded as new: replay beyond the window
    // is unsupported, and the run's record says so.
    let replay = intake.accept(tool_use("e0", "toolu_0", "a.txt"));
    assert_eq!(replay.records.len(), 1);
    let closing = intake.finish(&End {
        exit: Some(Exit::default()),
        stopped: None,
        unresolved: false,
    });
    let note = closing.records[0].text.clone().unwrap_or_default();
    assert!(
        note.contains("replay window that no longer covers every earlier record"),
        "{note}"
    );
}
