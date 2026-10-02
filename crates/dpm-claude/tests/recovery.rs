#![cfg(test)]
//! Recovery end to end: a host that died, the evidence a recovery needs before it touches a run, and
//! what it records.

#[path = "adapter/drive.rs"]
mod drive;
#[path = "adapter/support.rs"]
mod support;

use dpm_claude::{
    CommandError, HostError, Hosted, ProviderRecord, execute_blocking, host_directory,
    start_blocking,
};
use dpm_engine::Command;
use dpm_model::{ActivityInput, ActivityKind, Observation, RunId, RunState};
use drive::{never, new_run, service};
use serde_json::json;
use std::{
    fs,
    io::Read,
    process::{Command as Process, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use support::{Fixture, HANDSHAKE, emit, finished, init_event, pilot, text};

/// A run whose adapter died: recorded, a few events, still working, and a host record that says
/// whatever `host` says.
fn orphan(fixture: &Fixture, session: &str, host: impl FnOnce(&mut Hosted)) -> RunId {
    let mut app = fixture.app();
    let started = start_blocking(&mut app, &new_run(session)).expect("start");
    let entries = (1..=3)
        .map(|index| ActivityInput {
            run: started.run,
            source_sequence: index,
            kind: ActivityKind::Progress,
            text: Some(format!("message m{index}: before the crash")),
            observed_at: None,
        })
        .collect();
    app.record_run_activity_blocking(dpm_app::RunActivityRequest {
        actor: service(),
        entries,
        base_lineage: started.lineage,
    })
    .expect("activity");
    let directory = host_directory(&fixture.database).expect("host directory");
    let mut hosted = Hosted::begin_fresh_blocking(&directory, session).expect("host record");
    host(&mut hosted);
    drop(hosted);
    started.run
}

fn recovering_provider(fixture: &Fixture) -> std::path::PathBuf {
    fixture.script(
        "provider.sh",
        &[
            HANDSHAKE.to_string(),
            emit(&init_event()),
            emit(&text("r1", "continuing")),
            emit(&finished(&json!([]))),
        ]
        .concat(),
    )
}

#[test]
fn a_recovery_closes_the_prior_run_with_an_unknown_fate_and_starts_a_new_turn_that_names_it() {
    let fixture = Fixture::new();
    let prior = orphan(&fixture, "sess-dead", |_| {});
    let before = fixture.project_state();
    let outcome = execute_blocking(
        &fixture.resume_options(&recovering_provider(&fixture), prior, &[]),
        &never(),
    )
    .expect("recover");
    assert_eq!(
        (outcome.recovered, outcome.report.terminal.state),
        (Some(prior), RunState::Completed)
    );

    let app = fixture.app();
    let closed = app.run_blocking(prior).expect("prior").data;
    assert_eq!(
        closed.state,
        RunState::Interrupted,
        "never Completed: its turn's fate is unknown"
    );
    assert!(
        closed
            .state_detail
            .as_deref()
            .is_some_and(|detail| detail.contains("fate is unknown"))
    );
    let gap = fixture.texts(prior).pop().expect("the gap note");
    assert!(
        gap.contains("gap-4")
            && gap.contains("cannot be replayed")
            && gap.contains("after source sequence 3"),
        "{gap}"
    );

    let next = app.run_blocking(outcome.run).expect("recovery run").data;
    assert_eq!(
        next.run.parent,
        Some(prior),
        "the new run names the one it recovers"
    );
    let session = next.run.session.expect("session");
    assert_eq!(
        (session.session.as_str(), session.turn.as_deref()),
        ("sess-dead", Some("2")),
        "the same provider session, a new turn identity"
    );
    assert_eq!(
        (next.run.observation, next.state),
        (Observation::Managed, RunState::Completed)
    );
    let args = fs::read_to_string(fixture.marker("args.txt")).expect("provider arguments");
    assert!(
        args.lines().any(|line| line == "--resume=sess-dead"),
        "{args}"
    );
    assert_eq!(
        fixture.project_state(),
        before,
        "recovery changes no project state"
    );
}

fn refused_recovery(fixture: &Fixture, prior: RunId) {
    let before = (
        fixture.texts(prior).len(),
        fixture.app().run_blocking(prior).expect("prior").data.state,
    );
    let refused = execute_blocking(
        &fixture.resume_options(&recovering_provider(fixture), prior, &[]),
        &never(),
    );
    assert!(
        matches!(refused, Err(CommandError::Recover { .. })),
        "{refused:?}"
    );
    let after = (
        fixture.texts(prior).len(),
        fixture.app().run_blocking(prior).expect("prior").data.state,
    );
    assert_eq!(after, before, "a refused recovery changes nothing");
    assert!(
        !fixture.marker("args.txt").exists(),
        "no provider was started"
    );
}

#[test]
fn a_recovery_refuses_a_live_host_and_every_record_that_does_not_prove_the_provider_stopped() {
    // A host that is still running.
    let fixture = Fixture::new();
    let prior = orphan(&fixture, "sess-live", |_| {});
    let directory = host_directory(&fixture.database).expect("directory");
    let live =
        Hosted::recover_blocking(&directory, "sess-live").expect("the test takes the host's place");
    refused_recovery(&fixture, prior);
    drop(live);

    // A host that was starting a provider when it died.
    let starting = Fixture::new();
    let prior = orphan(&starting, "sess-starting", |host| {
        host.record(ProviderRecord::Starting).expect("record")
    });
    refused_recovery(&starting, prior);

    // A provider that is still running.
    let running = Fixture::new();
    let mut sleeper = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("sleep");
    let prior = orphan(&running, "sess-running", |host| {
        host.record(ProviderRecord::Running(sleeper.id()))
            .expect("record")
    });
    refused_recovery(&running, prior);
    sleeper.kill().expect("kill");
    sleeper.wait().expect("reap");

    // A corrupt record, and no record at all.
    let corrupt = Fixture::new();
    let prior = orphan(&corrupt, "sess-corrupt", |_| {});
    fs::write(
        host_directory(&corrupt.database)
            .expect("directory")
            .join("sess-corrupt.lock"),
        b"{\"v\":1,",
    )
    .expect("corrupt");
    refused_recovery(&corrupt, prior);
    let missing = Fixture::new();
    let prior = orphan(&missing, "sess-missing", |_| {});
    fs::remove_file(
        host_directory(&missing.database)
            .expect("directory")
            .join("sess-missing.lock"),
    )
    .expect("remove");
    refused_recovery(&missing, prior);
}

#[test]
fn a_recovery_refuses_a_task_that_is_no_longer_being_executed_and_a_run_that_already_ended() {
    let fixture = Fixture::new();
    let prior = orphan(&fixture, "sess-blocked", |_| {});
    fixture.project_command(
        &pilot(),
        Command::Block {
            work: fixture.work,
            reason: "waiting".into(),
        },
    );
    let refused = execute_blocking(
        &fixture.resume_options(&recovering_provider(&fixture), prior, &[]),
        &never(),
    );
    assert!(
        matches!(refused, Err(CommandError::Task { .. })),
        "{refused:?}"
    );
    assert!(!fixture.marker("args.txt").exists());

    let done = Fixture::new();
    let finished_run = execute_blocking(&done.options(&recovering_provider(&done), &[]), &never())
        .expect("a run")
        .run;
    let again = execute_blocking(
        &done.resume_options(&recovering_provider(&done), finished_run, &[]),
        &never(),
    );
    assert!(
        matches!(again, Err(CommandError::Recover { .. })),
        "{again:?}"
    );
}

#[test]
fn a_host_refusal_for_a_new_session_is_typed() {
    let fixture = Fixture::new();
    let directory = host_directory(&fixture.database).expect("directory");
    let held = Hosted::begin_fresh_blocking(&directory, "taken").expect("host");
    assert!(matches!(
        Hosted::begin_fresh_blocking(&directory, "taken"),
        Err(HostError::Exists { .. })
    ));
    drop(held);
}

/// Runs the real binary against a provider that ignores its input, and returns once the provider
/// has come up: it writes to a FIFO only after it was sent its prompt, which only happens once the
/// run is recorded, so the blocking read is the event that says the run is up.
fn start_hosted_adapter(fixture: &Fixture) -> std::process::Child {
    let fifo = fixture.marker("up.fifo");
    assert!(
        Process::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo")
            .success()
    );
    let script = fixture.script(
        "provider.sh",
        &[
            HANDSHAKE.to_string(),
            emit(&init_event()),
            "echo up > up.fifo\nexec sleep 120\n".to_string(),
        ]
        .concat(),
    );
    let adapter = Process::new(env!("CARGO_BIN_EXE_dpm-claude"))
        .args([
            "run",
            "--work",
            "TEST-A",
            "--executor",
            "agent:pilot",
            "--prompt",
            "check",
            "--max-seconds",
            "300",
        ])
        .arg("--directory")
        .arg(&fixture.work_dir)
        .arg("--database")
        .arg(&fixture.database)
        .arg("--claude")
        .arg(&script)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the adapter");
    let (up, came_up) = mpsc::channel();
    thread::spawn(move || {
        let mut text = String::new();
        let read = fs::File::open(fifo).and_then(|mut file| file.read_to_string(&mut text));
        up.send(read.map(|_| text)).ok();
    });
    came_up
        .recv_timeout(Duration::from_secs(30))
        .expect("the provider came up")
        .expect("the FIFO read");
    adapter
}

/// The process the dead adapter's host record says it left running.
fn recorded_provider_pid(fixture: &Fixture) -> u32 {
    let records = host_directory(&fixture.database).expect("directory");
    let record = fs::read_dir(&records)
        .expect("host records")
        .next()
        .expect("one record")
        .expect("entry")
        .path();
    let recorded: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(record).expect("record")).expect("json");
    recorded["provider"]["pid"]
        .as_u64()
        .expect("the provider's process") as u32
}

/// Stops a process and waits until it is really gone.
fn kill_and_wait_until_gone(pid: u32) {
    assert!(
        Process::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .expect("kill")
            .success()
    );
    let gone = Instant::now() + Duration::from_secs(15);
    while Process::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .expect("kill -0")
        .success()
    {
        assert!(Instant::now() < gone, "the provider did not exit");
        thread::yield_now();
    }
}

#[test]
fn a_real_adapter_killed_mid_run_leaves_a_live_provider_that_blocks_recovery_until_it_is_gone() {
    // The real binary hosts a provider that ignores its input; the adapter is then killed outright,
    // as a crash would. Its provider outlives it, so the recovery must refuse, and only once the
    // provider is really gone may it close the run honestly and record the gap.
    let fixture = Fixture::new();
    let mut adapter = start_hosted_adapter(&fixture);
    adapter.kill().expect("kill the adapter");
    adapter.wait().expect("reap the adapter");

    let prior = fixture
        .app()
        .runs_blocking(&dpm_app::RunQuery {
            key: Some("TEST-A".into()),
            limit: 10,
        })
        .expect("runs")
        .data
        .runs
        .remove(0);
    assert_eq!(
        prior.state,
        RunState::Working,
        "the dead host left the run open, not ended"
    );
    let provider_pid = recorded_provider_pid(&fixture);

    let second = fixture.script(
        "second.sh",
        &[
            "touch second-started\n".to_string(),
            HANDSHAKE.to_string(),
            emit(&init_event()),
            emit(&finished(&json!([]))),
        ]
        .concat(),
    );
    let resume = || {
        execute_blocking(
            &fixture.resume_options(&second, prior.run.id, &[]),
            &never(),
        )
    };
    let refused = resume();
    assert!(
        matches!(
            refused,
            Err(CommandError::Recover {
                source: Some(_),
                ..
            })
        ),
        "the refusal keeps the host's failure as its source: {refused:?}"
    );
    assert!(
        !fixture.marker("second-started").exists(),
        "no second provider was started beside the live one"
    );
    let still = fixture.app().run_blocking(prior.run.id).expect("prior");
    assert_eq!(still.data.state, RunState::Working);

    kill_and_wait_until_gone(provider_pid);
    let outcome = resume().expect("recover");
    assert_eq!(
        (outcome.recovered, outcome.report.terminal.state),
        (Some(prior.run.id), RunState::Completed)
    );
    let closed = fixture.app().run_blocking(prior.run.id).expect("prior");
    assert_eq!(closed.data.state, RunState::Interrupted);
    assert!(
        fixture
            .texts(prior.run.id)
            .iter()
            .any(|text| text.contains("cannot be replayed")),
        "the gap is recorded"
    );
}
