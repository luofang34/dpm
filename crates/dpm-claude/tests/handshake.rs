#![cfg(test)]
//! The handshake gate end to end: nothing is recorded, and nothing is asked of the provider, unless
//! it acknowledges `initialize` with a matching success response. Each provider here is a real
//! process; the sentinel is a file it creates only if it is ever sent a prompt.

#[path = "adapter/drive.rs"]
mod drive;
#[path = "adapter/support.rs"]
mod support;

use dpm_claude::{AppSink, BeginError, CommandError, DriveFailure, HandshakeError, drive_blocking};
use drive::{never, provider, turn};
use serde_json::json;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use support::{Fixture, HANDSHAKE, emit, finished, init_event, text};

/// Wait for a prompt and note it: reached only if the adapter sends one.
const SENTINEL: &str = "read user_prompt && touch prompt-seen\nexec sleep 30\n";

const READ_INITIALIZE: &str = "read initialize_request\n";

fn ack(request: &str, subtype: &str) -> String {
    format!(
        "printf '%s\\n' '{{\"type\":\"control_response\",\"response\":{{\"subtype\":\"{subtype}\",\"request_id\":\"{request}\",\"response\":{{}}}}}}'\n"
    )
}

/// Drive `body` as a provider, and check that nothing was recorded or dispatched.
fn refused(body: &str, deadline: Duration) -> DriveFailure {
    let fixture = Fixture::new();
    let before = fixture.project_state();
    let script = fixture.script("provider.sh", body);
    let begun = AtomicBool::new(false);
    let failure = drive_blocking::<AppSink>(
        provider(&fixture, &script, "s1"),
        &turn("s1", deadline),
        |_| {
            begun.store(true, Ordering::SeqCst);
            Err(BeginError::new("the test", "the run must not be recorded"))
        },
        &never(),
    )
    .expect_err("the handshake was never acknowledged");
    assert!(!begun.load(Ordering::SeqCst), "no run was begun");
    assert!(
        !fixture.marker("prompt-seen").exists(),
        "the prompt was sent to a provider that never acknowledged"
    );
    assert!(failure.stopped.reaped, "{failure:?}");
    let runs = fixture
        .app()
        .runs_blocking(&dpm_app::RunQuery {
            key: Some("TEST-A".into()),
            limit: 10,
        })
        .expect("runs")
        .data
        .runs;
    assert!(runs.is_empty(), "no run was recorded");
    assert_eq!(fixture.project_state(), before);
    failure
}

fn handshake(failure: &DriveFailure) -> &HandshakeError {
    failure
        .error
        .source
        .downcast_ref::<HandshakeError>()
        .unwrap_or_else(|| panic!("not a handshake failure: {failure:?}"))
}

#[test]
fn a_flood_of_unrelated_frames_never_stands_in_for_the_acknowledgement() {
    let noise = emit(&text("e1", "noise")).repeat(100);
    let failure = refused(
        &[READ_INITIALIZE, &noise, SENTINEL].concat(),
        Duration::from_secs(20),
    );
    assert!(
        matches!(handshake(&failure), HandshakeError::Silent { .. }),
        "{failure:?}"
    );
}

#[test]
fn a_session_announced_without_the_acknowledgement_never_dispatches_the_task() {
    let failure = refused(
        &[READ_INITIALIZE, &emit(&init_event()), SENTINEL].concat(),
        Duration::from_secs(20),
    );
    assert!(
        matches!(handshake(&failure), HandshakeError::Protocol(reason) if reason.contains("before it acknowledged")),
        "{failure:?}"
    );
}

#[test]
fn an_answer_to_another_request_or_in_another_envelope_is_not_the_acknowledgement() {
    // An answer to some other request, an answer in the wrong kind of envelope, and a refusal: none
    // acknowledges this adapter's `initialize`, so the time bound ends each.
    let wrong_request = ack("someone-elses", "success");
    let wrong_envelope = emit(&json!({"type": "assistant", "response": {
        "subtype": "success", "request_id": "dpm-init-1"}}));
    for answer in [wrong_request, wrong_envelope] {
        let failure = refused(
            &[READ_INITIALIZE, &answer, SENTINEL].concat(),
            Duration::from_secs(1),
        );
        assert!(
            matches!(handshake(&failure), HandshakeError::Deadline),
            "{failure:?}"
        );
    }
}

#[test]
fn a_refused_initialize_and_a_provider_that_ends_are_each_their_own_failure() {
    let refusal = refused(
        &[READ_INITIALIZE, &ack("dpm-init-1", "error"), SENTINEL].concat(),
        Duration::from_secs(20),
    );
    assert!(
        matches!(handshake(&refusal), HandshakeError::Protocol(reason) if reason.contains("refused")),
        "{refusal:?}"
    );
    let ended = refused(READ_INITIALIZE, Duration::from_secs(20));
    assert!(
        matches!(handshake(&ended), HandshakeError::Ended),
        "{ended:?}"
    );
}

#[test]
fn the_command_reports_an_unacknowledged_handshake_as_unrecorded_and_clears_its_host() {
    let fixture = Fixture::new();
    let script = fixture.script(
        "provider.sh",
        &[READ_INITIALIZE, &emit(&init_event()), SENTINEL].concat(),
    );
    let result = dpm_claude::execute_blocking(&fixture.options(&script, &[]), &never());
    assert!(
        matches!(&result, Err(CommandError::Begin { stopped, .. }) if stopped.reaped),
        "{result:?}"
    );
    assert!(!fixture.marker("prompt-seen").exists());
}

#[test]
fn an_acknowledged_provider_that_announces_only_after_the_prompt_still_runs() {
    // The legitimate delayed-announcement path of the installed runtime: acknowledged first,
    // announced only after the prompt, which was sent only once the run was recorded.
    let fixture = Fixture::new();
    let script = fixture.script(
        "provider.sh",
        &[
            HANDSHAKE.to_string(),
            emit(&init_event()),
            emit(&finished(&json!([]))),
        ]
        .concat(),
    );
    let outcome =
        dpm_claude::execute_blocking(&fixture.options(&script, &[]), &never()).expect("a run");
    assert_eq!(
        outcome.report.terminal.state,
        dpm_model::RunState::Completed
    );
    assert!(fixture.marker("prompt-seen").exists());
}
