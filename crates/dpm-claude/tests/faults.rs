#![cfg(test)]
//! Faults and bounds end to end: a store that fails, a provider that ignores its input, a
//! cancellation, and project commands that must never wait on any of it.

#[path = "adapter/drive.rs"]
mod drive;
#[path = "adapter/support.rs"]
mod support;

use dpm_claude::{AppSink, BeginError, Evidence, SinkError, drive_blocking};
use dpm_engine::Command;
use dpm_model::RunState;
use drive::{five_events, flaky, hooked, never, provider, provider_with_queue, turn};
use serde_json::json;
use std::{
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};
use support::{Fixture, HANDSHAKE, emit, finished, init_event, pilot};

#[test]
fn a_busy_store_is_retried_under_the_same_identities_and_nothing_is_lost_or_doubled() {
    let fixture = Fixture::new();
    let script = fixture.script("provider.sh", &five_events());
    let report = drive_blocking(
        provider(&fixture, &script, "s1"),
        &turn("s1", Duration::from_secs(20)),
        flaky(&fixture, "s1", 3, false, false),
        &never(),
    )
    .expect("drive");
    assert_eq!(report.terminal.state, RunState::Completed);
    assert!(report.terminal_recorded && report.sink_failure.is_none());
    let sequences: Vec<u64> = fixture
        .activity(report.run)
        .iter()
        .map(|entry| entry.record.source_sequence)
        .collect();
    assert_eq!(
        sequences,
        (1..=6).collect::<Vec<u64>>(),
        "one session record and five steps, contiguous, none doubled"
    );
}

#[test]
fn recording_that_cannot_succeed_stops_the_provider_and_is_reported_with_its_cause() {
    let fixture = Fixture::new();
    let script = fixture.script(
        "provider.sh",
        &[
            HANDSHAKE.to_string(),
            emit(&init_event()),
            "exec sleep 30\n".to_string(),
        ]
        .concat(),
    );
    let started = Instant::now();
    let report = drive_blocking(
        provider(&fixture, &script, "s2"),
        &turn("s2", Duration::from_secs(20)),
        flaky(&fixture, "s2", 0, true, false),
        &never(),
    )
    .expect("drive");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "the provider was stopped, not waited for"
    );
    assert_eq!(
        (report.terminal.state, report.terminal.evidence),
        (RunState::Interrupted, Evidence::AdapterStopped)
    );
    assert!(!report.terminal_recorded);
    let failure = report
        .sink_failure
        .as_ref()
        .expect("the failure is reported");
    assert!(
        matches!(failure, SinkError::Refused { .. })
            && failure.chain().contains("the store refuses"),
        "{}",
        failure.chain()
    );
    assert!(report.stopped.killed && report.stopped.reaped);
    // The run was never closed: it stays Working, and silence will show as stale, not finished.
    let states: Vec<RunState> = fixture
        .lifecycle(report.run)
        .iter()
        .map(|entry| entry.event.state)
        .collect();
    assert_eq!(states, [RunState::Working]);
}

#[test]
fn a_terminal_write_that_fails_leaves_the_run_working_and_says_so() {
    let fixture = Fixture::new();
    let script = fixture.script("provider.sh", &five_events());
    let report = drive_blocking(
        provider(&fixture, &script, "s3"),
        &turn("s3", Duration::from_secs(20)),
        flaky(&fixture, "s3", 0, false, true),
        &never(),
    )
    .expect("drive");
    assert_eq!(
        report.terminal.state,
        RunState::Completed,
        "what the evidence says"
    );
    assert!(
        !report.terminal_recorded && report.sink_failure.is_some(),
        "but it was not recorded, and that is reported"
    );
    assert_eq!(fixture.lifecycle(report.run).len(), 1);
}

#[test]
fn the_deadline_ends_a_provider_that_ignores_its_input_within_bounds() {
    let fixture = Fixture::new();
    let script = fixture.script(
        "provider.sh",
        &[HANDSHAKE.to_string(), "exec sleep 30\n".to_string()].concat(),
    );
    let started = Instant::now();
    let report = drive_blocking(
        provider(&fixture, &script, "s4"),
        &turn("s4", Duration::from_secs(1)),
        flaky(&fixture, "s4", 0, false, false),
        &never(),
    )
    .expect("drive");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        (report.terminal.state, report.terminal.evidence),
        (RunState::Interrupted, Evidence::AdapterStopped)
    );
    assert!(
        report.terminal.detail.contains("time bound")
            && report.terminal_recorded
            && report.stopped.killed
    );
}

#[test]
fn a_cancel_request_stops_the_provider_and_is_its_own_evidence() {
    // The cancel is raised from inside the first write to the store, which only happens once the
    // provider has announced itself: the trigger is the provider's own event, and without it the
    // run would end at its time bound with different evidence, failing the assertions below.
    let fixture = Fixture::new();
    let script = fixture.script(
        "provider.sh",
        &[
            HANDSHAKE.to_string(),
            emit(&init_event()),
            "exec sleep 30\n".to_string(),
        ]
        .concat(),
    );
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let trigger = cancel.clone();
    let begin = hooked(&fixture, "s5", move |sink| {
        sink.cancel_after_first = Some(trigger);
    });
    let report = drive_blocking(
        provider(&fixture, &script, "s5"),
        &turn("s5", Duration::from_secs(30)),
        begin,
        &cancel,
    )
    .expect("drive");
    assert_eq!(
        (report.terminal.state, report.terminal.evidence),
        (RunState::Interrupted, Evidence::AdapterStopped)
    );
    assert!(
        report.terminal.detail.contains("on request") && report.terminal_recorded,
        "{}",
        report.terminal.detail
    );
    assert!(
        fixture.texts(report.run)[0].starts_with("session "),
        "the provider's announcement was what triggered it"
    );
}

#[test]
fn the_prompt_is_never_sent_when_the_run_cannot_be_recorded() {
    let fixture = Fixture::new();
    let script = fixture.script("provider.sh", HANDSHAKE);
    let refused = drive_blocking::<AppSink>(
        provider(&fixture, &script, "s6"),
        &turn("s6", Duration::from_secs(10)),
        |_| {
            Err(BeginError::new(
                "recording the run",
                "the task was released between the check and the start",
            ))
        },
        &never(),
    );
    let failure = refused.expect_err("the run could not be recorded");
    assert!(
        failure.stopped.reaped && failure.error.source.to_string().contains("released"),
        "{failure:?}"
    );
    assert!(
        !fixture.marker("prompt-seen").exists(),
        "nothing was asked of the provider"
    );
}

/// A provider that floods half its events, waits on a FIFO the test controls, then floods the rest.
fn held_flood_script(fixture: &Fixture) -> (std::path::PathBuf, std::path::PathBuf) {
    let fifo = fixture.marker("release.fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo")
            .success()
    );
    let burst = |from: u32, to: u32| {
        format!(
            "i={from}\nwhile [ $i -lt {to} ]; do printf '%s\\n' '{{\"type\":\"assistant\",\"uuid\":\"u'$i'\",\"session_id\":\"'\"$SESSION\"'\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"id\":\"toolu_'$i'\",\"name\":\"Read\",\"input\":{{\"file_path\":\"f.txt\"}}}}]}}}}'; i=$((i+1)); done\n"
        )
    };
    let script = fixture.script(
        "provider.sh",
        &[
            HANDSHAKE.to_string(),
            emit(&init_event()),
            burst(0, 1000),
            "read go < release.fifo\n".to_string(),
            burst(1000, 2000),
            emit(&finished(&json!([]))),
        ]
        .concat(),
    );
    (script, fifo)
}

#[test]
fn a_flood_never_queues_beyond_its_bound_and_project_commands_commit_while_it_is_held_open() {
    // The provider emits half a flood, then waits on a FIFO the test controls, so the run is
    // demonstrably in progress, with its activity recorded, while project commands commit.
    let fixture = Fixture::new();
    let (script, fifo) = held_flood_script(&fixture);
    let (held, at_barrier) = std::sync::mpsc::channel();
    let begin = hooked(&fixture, "s7", move |sink| {
        // The session record and the first thousand events.
        sink.notify = Some((1001, held));
    });
    let work = fixture.work;
    let before = fixture.project_state();
    let report = thread::scope(|scope| {
        let adapter = scope.spawn(|| {
            drive_blocking(
                provider_with_queue(&fixture, &script, "s7", 4),
                &turn("s7", Duration::from_secs(60)),
                begin,
                &never(),
            )
        });
        at_barrier
            .recv_timeout(Duration::from_secs(30))
            .expect("the adapter recorded the first half of the flood");
        // The adapter is mid-run: the run is working and holds activity.
        let app = fixture.app();
        let runs = app
            .runs_blocking(&dpm_app::RunQuery {
                key: Some("TEST-A".into()),
                limit: 10,
            })
            .expect("runs")
            .data
            .runs;
        assert_eq!(
            (runs[0].state, runs[0].activity.recorded),
            (RunState::Working, 1001)
        );
        for percent in 1..=20_u8 {
            fixture.project_command(
                &pilot(),
                Command::ReportProgress {
                    work,
                    percent,
                    note: None,
                },
            );
        }
        std::fs::write(&fifo, "go\n").expect("release the provider");
        adapter
            .join()
            .expect("the adapter thread")
            .expect("the run")
    });
    assert_eq!(report.terminal.state, RunState::Completed);
    assert_eq!(
        report.tally.recorded, 2001,
        "every event arrived, none was dropped to make room"
    );
    assert!(
        report.pending_high_water <= 6,
        "{}",
        report.pending_high_water
    );
    assert_eq!(
        fixture.project_state().0,
        before.0 + 20,
        "exactly the project commands moved the revision"
    );
}
