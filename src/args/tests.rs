use super::*;
use clap::CommandFactory;

const RUN: &str = "0192f000-0000-7000-8000-0000000000a1";

fn parse(arguments: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("dpm").chain(arguments.iter().copied()))
}

fn run_command(arguments: &[&str]) -> RunCommand {
    let mut line = vec!["run"];
    line.extend_from_slice(arguments);
    match parse(&line).expect("parses").command {
        Some(Commands::Store(StoreCommand::Run { command })) => command,
        other => panic!("not a run command: {other:?}"),
    }
}

#[test]
fn the_whole_command_tree_is_well_formed() {
    // Builds every command, argument and relation and checks them the way clap does in debug.
    Cli::command().debug_assert();
}

#[test]
fn run_commands_read_back_what_was_given() {
    let RunCommand::Record {
        run,
        kind,
        source_sequence,
        text,
        actor,
        ..
    } = run_command(&[
        "record",
        RUN,
        "tool_result",
        "--sequence",
        "7",
        "--text",
        "done",
        "--actor",
        "agent:worker",
    ])
    else {
        panic!("a record command");
    };
    assert_eq!(run.to_string(), RUN);
    assert_eq!(kind, dpm_model::ActivityKind::ToolResult);
    assert_eq!(
        (source_sequence, text.as_deref(), actor.as_str()),
        (7, Some("done"), "agent:worker")
    );

    let RunCommand::Start(start) = run_command(&[
        "start",
        "TEST-A",
        "--actor",
        "agent:worker",
        "--provider",
        "codex",
        "--session",
        "thread-1",
        "--source-commit",
        "0192f000-0000-4000-8000-0000000000aa=cccccccccccccccccccccccccccccccccccccccc",
    ]) else {
        panic!("a start command");
    };
    assert_eq!(start.observation, dpm_model::Observation::ReportedOnly);
    assert_eq!(start.source_commits.len(), 1);
    assert_eq!(start.provider.as_deref(), Some("codex"));

    let RunCommand::Activity {
        after_sequence,
        limit,
        run,
    } = run_command(&["activity", "--after-sequence", "9"])
    else {
        panic!("an activity feed");
    };
    assert_eq!((after_sequence, limit, run), (9, 100, None));
}

#[test]
fn run_commands_refuse_malformed_arguments() {
    for arguments in [
        // The activity identity is the reporter's sequence, which is required.
        vec!["run", "record", RUN, "heartbeat", "--actor", "agent:worker"],
        vec![
            "run",
            "record",
            RUN,
            "heartbeat",
            "--key",
            "k",
            "--sequence",
            "1",
            "--actor",
            "agent:worker",
        ],
        vec![
            "run",
            "record",
            RUN,
            "telemetry",
            "--sequence",
            "1",
            "--actor",
            "agent:worker",
        ],
        vec!["run", "report", RUN, "done", "--actor", "agent:worker"],
        vec![
            "run",
            "report",
            "not-a-uuid",
            "failed",
            "--actor",
            "agent:worker",
        ],
        vec![
            "run",
            "start",
            "TEST-A",
            "--actor",
            "agent:worker",
            "--provider",
            "codex",
        ],
        vec![
            "run",
            "start",
            "TEST-A",
            "--actor",
            "agent:worker",
            "--observation",
            "watched",
        ],
        vec!["run", "link", RUN, "--actor", "agent:worker"],
        vec!["run"],
        vec!["run", "unknown"],
    ] {
        assert!(parse(&arguments).is_err(), "{arguments:?}");
    }
}

#[test]
fn mcp_takes_the_global_selection_and_names_its_principal() {
    let cli = parse(&["--db", "x.sqlite", "mcp", "--actor", "agent:coder"]).expect("parses");
    assert_eq!(cli.database, Some(PathBuf::from("x.sqlite")));
    assert!(
        matches!(cli.command, Some(Commands::Open(OpenCommand::Mcp { ref actor })) if actor == "agent:coder"),
        "{:?}",
        cli.command
    );
    assert!(parse(&["mcp"]).is_err(), "a server without a principal");
    let cli = parse(&["mcp", "--project", "here", "--actor", "human:ada"]).expect("parses");
    assert_eq!(cli.project, Some(PathBuf::from("here")));
    let mut command = Cli::command();
    let help = command
        .find_subcommand_mut("mcp")
        .expect("mcp")
        .render_help()
        .to_string();
    for fragment in [
        "--actor <KIND:NAME>",
        "human:NAME, agent:NAME or service:NAME",
    ] {
        assert!(help.contains(fragment), "missing {fragment:?} in\n{help}");
    }
}
