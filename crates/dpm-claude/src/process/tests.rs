use super::*;
use std::time::Instant;

fn launch(script: &str) -> Launch {
    Launch {
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".into(), script.into()],
        directory: std::env::temp_dir(),
        remove_env: Vec::new(),
    }
}

/// Wait, without sleeping, until `done` holds or ten seconds pass.
fn until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "the condition never held");
        thread::yield_now();
    }
}

fn collect(provider: &Provider) -> Vec<Message> {
    let mut found = Vec::new();
    loop {
        match provider.next_blocking(Duration::from_secs(10)) {
            Received::Message(Message::End) => {
                found.push(Message::End);
                return found;
            }
            Received::Message(message) => found.push(message),
            Received::Timeout => panic!("the provider produced neither output nor an end"),
        }
    }
}

fn lines(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::Frame(Frame::Line(line)) => Some(line.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn output_is_framed_in_order_and_ends_cleanly_with_the_exit_reported() {
    let mut provider =
        Provider::spawn_blocking(&launch("printf 'a\\nb\\n'"), 8, 1024).expect("spawn");
    let messages = collect(&provider);
    assert_eq!(lines(&messages), ["a", "b"]);
    let stopped = provider.stop_blocking(Duration::from_secs(2));
    assert_eq!(
        (stopped.reaped, stopped.killed, stopped.exit.code),
        (true, false, Some(0))
    );
}

#[test]
fn an_oversized_line_is_reported_and_the_next_one_survives() {
    let script = "head -c 5000 /dev/zero | tr '\\0' x; echo; echo ok";
    let mut provider = Provider::spawn_blocking(&launch(script), 8, 1024).expect("spawn");
    let messages = collect(&provider);
    assert!(
        matches!(
            messages.first(),
            Some(Message::Frame(Frame::Oversized { bytes: 5000 }))
        ),
        "{messages:?}"
    );
    assert_eq!(lines(&messages), ["ok"]);
    provider.stop_blocking(Duration::from_secs(2));
}

#[test]
fn a_flood_never_queues_beyond_its_bound_and_loses_nothing() {
    let script = "i=0; while [ $i -lt 20000 ]; do echo \"line $i\"; i=$((i+1)); done";
    let mut provider = Provider::spawn_blocking(&launch(script), 4, 1024).expect("spawn");
    // Consume nothing until the reader has filled the queue: it is then blocked, and the provider
    // with it, which is the backpressure.
    until(|| provider.pending_high_water() >= 5);
    let messages = collect(&provider);
    let expected: Vec<String> = (0..20_000).map(|i| format!("line {i}")).collect();
    assert_eq!(lines(&messages), expected, "every line arrived, in order");
    assert!(
        provider.pending_high_water() <= 6,
        "the queue held {} messages",
        provider.pending_high_water()
    );
    provider.stop_blocking(Duration::from_secs(2));
}

#[test]
fn a_provider_that_never_reads_cannot_hold_a_write_past_its_bound() {
    let mut provider = Provider::spawn_blocking(&launch("exec sleep 30"), 4, 1024).expect("spawn");
    let line = "x".repeat(1023);
    let mut accepted = 0_usize;
    let started = Instant::now();
    let failure = loop {
        match provider.send_line_blocking(&line, Duration::from_millis(300), &|| false) {
            Ok(()) => accepted += 1,
            Err(error) => break error,
        }
        assert!(accepted < 1_000, "the pipe never filled");
    };
    assert!(
        matches!(failure, ProcessError::Timeout),
        "a full pipe ends in a timeout, not a refusal of the budget: {failure}"
    );
    assert!(accepted > 0, "the pipe takes some before it fills");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the write gave up within its bound"
    );
    let stopped = provider.stop_blocking(Duration::from_millis(200));
    assert!(stopped.killed && stopped.reaped, "{stopped:?}");
}

#[test]
fn a_write_blocked_on_a_full_pipe_is_abandoned_when_the_run_is_stopping() {
    let mut provider = Provider::spawn_blocking(&launch("exec sleep 30"), 4, 1024).expect("spawn");
    let line = "y".repeat(1023);
    // The stop is signalled only once the pipe is full and the write is waiting on it: the
    // abandon check is consulted on every slice of the wait.
    let waiting_since = std::cell::Cell::new(None::<Instant>);
    let stopping = || {
        let began = waiting_since.get().unwrap_or_else(Instant::now);
        waiting_since.set(Some(began));
        began.elapsed() > Duration::from_millis(300)
    };
    let failure = loop {
        match provider.send_line_blocking(&line, Duration::from_secs(30), &stopping) {
            Ok(()) => waiting_since.set(None),
            Err(error) => break error,
        }
    };
    assert!(matches!(failure, ProcessError::Abandoned), "{failure}");
    provider.stop_blocking(Duration::from_millis(200));
}

#[test]
fn the_input_budget_refuses_what_a_run_may_not_send() {
    let mut provider =
        Provider::spawn_blocking(&launch("exec cat >/dev/null"), 4, 1024).expect("spawn");
    provider.limit_input(100);
    provider
        .send_line_blocking(&"a".repeat(60), Duration::from_secs(5), &|| false)
        .expect("within the budget");
    assert!(matches!(
        provider.send_line_blocking(&"b".repeat(60), Duration::from_secs(5), &|| false),
        Err(ProcessError::Budget { budget: 100 })
    ));
    let stopped = provider.stop_blocking(Duration::from_secs(5));
    assert!(
        stopped.reaped && !stopped.killed,
        "cat exits when its input closes: {stopped:?}"
    );
}

#[test]
fn a_stubborn_provider_is_killed_after_its_grace_and_the_kill_is_reported() {
    let mut provider = Provider::spawn_blocking(&launch("exec sleep 30"), 4, 1024).expect("spawn");
    let stopped = provider.stop_blocking(Duration::from_millis(200));
    assert_eq!(
        (stopped.killed, stopped.reaped, stopped.exit.signal),
        (true, true, Some(9))
    );
    // Stopping again is harmless and reports the same.
    assert!(provider.stop_blocking(Duration::from_millis(50)).killed);
}

#[test]
fn a_descendant_holding_the_pipes_cannot_keep_a_reader_or_the_stop_waiting() {
    // The provider prints one line and exits, but what it started keeps its output pipe open for a
    // while, so end of input never arrives.
    let mut provider =
        Provider::spawn_blocking(&launch("sleep 3 & echo hello"), 4, 1024).expect("spawn");
    match provider.next_blocking(Duration::from_secs(10)) {
        Received::Message(Message::Frame(Frame::Line(line))) => assert_eq!(line, "hello"),
        other => panic!("{other:?}"),
    }
    until(|| provider.has_exited());
    let started = Instant::now();
    let stopped = provider.stop_blocking(Duration::from_millis(200));
    assert!(stopped.reaped && stopped.output_open, "{stopped:?}");
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "the stop did not wait for the descendant"
    );
}

#[test]
fn standard_error_is_counted_and_never_kept() {
    let mut provider = Provider::spawn_blocking(
        &launch("echo diagnostic-with-a-secret >&2; echo out"),
        4,
        1024,
    )
    .expect("spawn");
    collect(&provider);
    provider.stop_blocking(Duration::from_secs(2));
    assert_eq!(
        provider.stderr_bytes(),
        "diagnostic-with-a-secret\n".len() as u64
    );
}

#[test]
fn a_missing_program_is_a_typed_spawn_error() {
    let launch = Launch {
        program: PathBuf::from("/nonexistent/provider"),
        args: Vec::new(),
        directory: std::env::temp_dir(),
        remove_env: Vec::new(),
    };
    assert!(matches!(
        Provider::spawn_blocking(&launch, 4, 1024),
        Err(ProcessError::Spawn { .. })
    ));
}

#[test]
fn an_abandoned_provider_is_not_left_running() {
    let provider = Provider::spawn_blocking(&launch("exec sleep 30"), 4, 1024).expect("spawn");
    let pid = provider.child.id();
    drop(provider);
    // The child was killed and reaped by the drop: the process no longer exists to be signalled.
    let alive = std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .expect("kill")
        .success();
    assert!(!alive, "the abandoned provider still runs");
}
