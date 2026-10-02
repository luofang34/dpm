use super::*;

fn parse(words: &[&str]) -> Result<Parsed, OptionsError> {
    Options::parse(words.iter().map(|word| (*word).to_string()))
}

fn run_options(extra: &[&str]) -> Options {
    let mut words = vec![
        "run",
        "--work",
        "TEST-A",
        "--executor",
        "agent:pilot",
        "--directory",
        "/tmp/d",
        "--prompt",
        "hi",
    ];
    words.extend_from_slice(extra);
    match parse(&words).expect("valid") {
        Parsed::Run(options) => *options,
        Parsed::Help => panic!("not help"),
    }
}

#[test]
fn a_run_names_its_task_executor_directory_and_prompt() {
    let options = run_options(&[]);
    assert_eq!(options.command, Command::Run);
    assert_eq!(options.recorder, ActorId::service("dpm-claude"));
    assert_eq!(options.tools, "Read");
    assert_eq!(options.permission_mode, "manual");
    assert!(!options.persist_session);
}

#[test]
fn missing_pieces_are_usage_errors() {
    for words in [
        vec![
            "run",
            "--executor",
            "agent:a",
            "--directory",
            "d",
            "--prompt",
            "p",
        ],
        vec!["run", "--work", "K", "--directory", "d", "--prompt", "p"],
        vec![
            "run",
            "--work",
            "K",
            "--executor",
            "agent:a",
            "--prompt",
            "p",
        ],
        vec![
            "run",
            "--work",
            "K",
            "--executor",
            "agent:a",
            "--directory",
            "d",
        ],
        vec!["resume", "--directory", "d", "--prompt", "p"],
        vec!["launch"],
        vec![],
    ] {
        assert!(
            matches!(parse(&words), Err(OptionsError::Usage(_))),
            "{words:?}"
        );
    }
}

#[test]
fn modes_that_approve_by_themselves_are_refused() {
    for mode in ["bypassPermissions", "acceptEdits", "auto"] {
        let words = [
            "run",
            "--work",
            "K",
            "--executor",
            "agent:a",
            "--directory",
            "d",
            "--prompt",
            "p",
            "--permission-mode",
            mode,
        ];
        assert_eq!(parse(&words), Err(OptionsError::ApprovingMode(mode.into())));
    }
}

#[test]
fn the_provider_is_always_confined_and_never_given_a_bypass() {
    let options = run_options(&[
        "--allow-tool",
        "Read(./note.txt)",
        "--model",
        "claude-sonnet-5-5",
    ]);
    let args: Vec<String> = options
        .provider_arguments("abc", false)
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    for needed in [
        "--safe-mode",
        "--restricted",
        "--permission-prompt-tool",
        "stdio",
        "--no-session-persistence",
        "--session-id=abc",
    ] {
        assert!(args.iter().any(|arg| arg == needed), "{needed} in {args:?}");
    }
    assert!(
        !args
            .iter()
            .any(|arg| arg.contains("bypass") || arg == "--dangerously-skip-permissions")
    );
    let resumed: Vec<String> = options
        .provider_arguments("abc", true)
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        resumed.iter().any(|arg| arg == "--resume=abc")
            && !resumed.iter().any(|arg| arg.starts_with("--session-id"))
    );
}

#[test]
fn every_bound_is_finite_so_no_option_can_ask_for_unbounded_memory_or_time() {
    for (flag, value) in [
        ("--queue", "0"),
        ("--queue", "4097"),
        ("--queue", "18446744073709551615"),
        ("--max-line-bytes", "1023"),
        ("--max-line-bytes", "8388609"),
        ("--max-line-bytes", "18446744073709551615"),
        ("--max-seconds", "0"),
        ("--max-seconds", "86401"),
        ("--max-seconds", "18446744073709551615"),
    ] {
        let words = [
            "run",
            "--work",
            "K",
            "--executor",
            "agent:a",
            "--directory",
            "d",
            "--prompt",
            "p",
            flag,
            value,
        ];
        assert!(
            matches!(parse(&words), Err(OptionsError::Usage(_))),
            "{flag} {value}"
        );
    }
    for (flag, value) in [
        ("--queue", "4096"),
        ("--max-line-bytes", "8388608"),
        ("--max-seconds", "86400"),
    ] {
        let words = [
            "run",
            "--work",
            "K",
            "--executor",
            "agent:a",
            "--directory",
            "d",
            "--prompt",
            "p",
            flag,
            value,
        ];
        assert!(
            parse(&words).is_ok(),
            "{flag} {value} is the largest accepted"
        );
    }
}

#[test]
fn a_resume_names_its_run() {
    let run = Uuid::now_v7().to_string();
    let words = [
        "resume",
        "--run",
        run.as_str(),
        "--directory",
        "d",
        "--prompt",
        "p",
    ];
    let Parsed::Run(options) = parse(&words).expect("valid") else {
        panic!("not help")
    };
    assert_eq!(
        options.command,
        Command::Resume(RunId(Uuid::parse_str(&run).expect("uuid")))
    );
}
