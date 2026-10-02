use super::*;
use std::process::{Child, Command, Stdio};

fn sleeper() -> Child {
    Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn sleep")
}

/// A host record as a crashed host might have left it: the lock is free, the content is whatever it was.
fn leave(directory: &Path, session: &str, content: &[u8]) {
    fs::create_dir_all(directory).expect("directory");
    fs::write(directory.join(format!("{session}.lock")), content).expect("record");
}

fn recovery(directory: &Path, session: &str) -> Result<Hosted, HostError> {
    Hosted::recover_blocking(directory, session)
}

#[test]
fn a_session_hosted_by_a_live_adapter_cannot_be_hosted_or_recovered_by_another() {
    let directory = tempfile::tempdir().expect("directory");
    let first = Hosted::begin_fresh_blocking(directory.path(), "a").expect("first host");
    assert!(matches!(
        recovery(directory.path(), "a"),
        Err(HostError::Active { .. })
    ));
    assert!(matches!(
        Hosted::begin_fresh_blocking(directory.path(), "a"),
        Err(HostError::Exists { .. })
    ));
    Hosted::begin_fresh_blocking(directory.path(), "b").expect("another session");
    drop(first);
}

#[test]
fn releasing_a_host_does_not_wait_for_copies_of_its_descriptor_to_close() {
    // A process forked by another thread holds a copy of every open descriptor until it execs, and
    // a lock belongs to the open file, not the descriptor: dropping the host must still release it
    // at once. A held copy stands in for that child, deterministically.
    let directory = tempfile::tempdir().expect("directory");
    let mut host = Hosted::begin_fresh_blocking(directory.path(), "s").expect("host");
    host.record(ProviderRecord::Nothing).expect("none running");
    let copy = host.file.try_clone().expect("a copy of the descriptor");
    drop(host);
    let recovered = recovery(directory.path(), "s");
    assert!(recovered.is_ok(), "{recovered:?}");
    drop(copy);
}

#[test]
fn a_cleanly_ended_host_can_be_recovered() {
    let directory = tempfile::tempdir().expect("directory");
    let mut host = Hosted::begin_fresh_blocking(directory.path(), "s").expect("host");
    host.record(ProviderRecord::Running(u32::MAX - 1))
        .expect("a process that does not exist");
    host.record(ProviderRecord::Nothing).expect("none running");
    drop(host);
    recovery(directory.path(), "s").expect("no provider can be running");
}

#[test]
fn a_missing_record_refuses_a_recovery() {
    let directory = tempfile::tempdir().expect("directory");
    assert!(matches!(
        recovery(directory.path(), "never-hosted"),
        Err(HostError::NoRecord { .. })
    ));
}

#[test]
fn a_corrupt_record_refuses_a_recovery_and_is_left_as_it_was() {
    let directory = tempfile::tempdir().expect("directory");
    let long = vec![b'x'; 600];
    for (name, content) in [
        ("empty", &b""[..]),
        (
            "truncated",
            &br#"{"v":1,"adapter_pid":7,"provider":{"state":"run"#[..],
        ),
        ("not-json", &b"hello"[..]),
        (
            "wrong-version",
            &br#"{"v":2,"adapter_pid":7,"provider":{"state":"none"}}"#[..],
        ),
        (
            "unknown-state",
            &br#"{"v":1,"adapter_pid":7,"provider":{"state":"asleep"}}"#[..],
        ),
        (
            "running-without-pid",
            &br#"{"v":1,"adapter_pid":7,"provider":{"state":"running"}}"#[..],
        ),
        (
            "running-with-zero-pid",
            &br#"{"v":1,"adapter_pid":7,"provider":{"state":"running","pid":0}}"#[..],
        ),
        ("not-utf8", &b"\xff\xfe\xfd"[..]),
        ("too-long", &long[..]),
    ] {
        leave(directory.path(), name, content);
        let refused = recovery(directory.path(), name);
        assert!(
            matches!(refused, Err(HostError::Corrupt { .. })),
            "{name}: {refused:?}"
        );
        assert_eq!(
            fs::read(directory.path().join(format!("{name}.lock"))).expect("record"),
            content,
            "{name} is not rewritten"
        );
    }
}

#[test]
fn a_host_that_died_while_starting_a_provider_refuses_a_recovery() {
    // The crash windows around starting a provider: before the identity was persisted, the record
    // says only that one was about to start, and that is enough to refuse.
    let directory = tempfile::tempdir().expect("directory");
    let mut host = Hosted::begin_fresh_blocking(directory.path(), "s").expect("host");
    host.record(ProviderRecord::Starting).expect("starting");
    drop(host);
    assert!(matches!(
        recovery(directory.path(), "s"),
        Err(HostError::ProviderStarting { .. })
    ));
}

#[test]
fn a_recorded_provider_that_still_exists_refuses_a_recovery_until_it_is_gone() {
    let directory = tempfile::tempdir().expect("directory");
    let mut provider = sleeper();
    let mut host = Hosted::begin_fresh_blocking(directory.path(), "s").expect("host");
    host.record(ProviderRecord::Running(provider.id()))
        .expect("record");
    drop(host);
    let refused = recovery(directory.path(), "s");
    assert!(
        matches!(refused, Err(HostError::ProviderMayStillRun { pid, .. }) if pid == provider.id()),
        "{refused:?}"
    );
    provider.kill().expect("kill");
    provider.wait().expect("reap");
    recovery(directory.path(), "s").expect("the recorded provider is shown to be gone");
}

#[test]
fn a_process_whose_existence_cannot_be_told_is_not_taken_for_gone() {
    // Process 1 exists and belongs to someone else: the answer is "exists" or "not permitted",
    // and neither proves absence.
    let directory = tempfile::tempdir().expect("directory");
    leave(
        directory.path(),
        "s",
        br#"{"v":1,"adapter_pid":7,"provider":{"state":"running","pid":1}}"#,
    );
    assert!(matches!(
        recovery(directory.path(), "s"),
        Err(HostError::ProviderMayStillRun { pid: 1, .. })
    ));
}

#[test]
fn only_names_a_file_can_safely_carry_are_sessions() {
    let directory = tempfile::tempdir().expect("directory");
    for bad in ["", "../escape", "a/b", "with space", &"x".repeat(101)] {
        assert!(
            matches!(
                Hosted::begin_fresh_blocking(directory.path(), bad),
                Err(HostError::BadSession(_))
            ),
            "{bad:?}"
        );
        assert!(
            matches!(
                recovery(directory.path(), bad),
                Err(HostError::BadSession(_))
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn the_record_location_follows_the_stores_canonical_identity_not_the_caller() {
    let directory = tempfile::tempdir().expect("directory");
    let store = directory.path().join("state.sqlite");
    fs::write(&store, b"").expect("store");
    let link = directory.path().join("link.sqlite");
    std::os::unix::fs::symlink(&store, &link).expect("symlink");
    let direct = super::directory(&store).expect("direct");
    assert_eq!(super::directory(&link).expect("through a link"), direct);
    // A different spelling of the same path, as another working directory would produce.
    let indirect = directory.path().join("a").join("..").join("state.sqlite");
    fs::create_dir_all(directory.path().join("a")).expect("dir");
    assert_eq!(super::directory(&indirect).expect("indirect"), direct);
    assert!(super::directory(&directory.path().join("absent.sqlite")).is_err());
}
