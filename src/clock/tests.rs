use super::*;
use crate::args::Cli;
use clap::Parser;

fn clock_for(arguments: &[&str]) -> Result<QueryClock, CliError> {
    let cli = Cli::try_parse_from(arguments).expect("arguments");
    let command = cli.command.expect("command");
    for_command(&command, cli.clock)
}

#[test]
fn queries_take_a_pinned_clock_and_mutations_refuse_it() {
    let at = "2030-01-02T03:04:05Z";
    let instant: DateTime<Utc> = at.parse().expect("instant");
    for query in [
        &["dpm", "--clock", at, "status"][..],
        &["dpm", "explain", "TEST-A", "--clock", at],
        &["dpm", "--clock", at, "next"],
        &["dpm", "--clock", at, "revision"],
        &["dpm", "--clock", at, "history"],
        &["dpm", "--clock", at, "plan", "diff", "candidate.json"],
    ] {
        let clock = clock_for(query).expect("query");
        assert_eq!(clock, QueryClock::Fixed(instant), "{query:?}");
    }
    for refused in [
        &[
            "dpm", "--clock", at, "claim", "TEST-A", "--actor", "agent:a",
        ][..],
        &[
            "dpm", "--clock", at, "plan", "apply", "c.json", "--reason", "r", "--actor", "human:h",
        ],
        &["dpm", "--clock", at, "import", "plan.json"],
        &["dpm", "--clock", at, "backup", "--to", "copy.sqlite"],
        &["dpm", "--clock", at, "tui"],
        &["dpm", "--clock", at, "validate", "plan.json"],
        &["dpm", "--clock", at, "plan", "schema"],
    ] {
        let error = clock_for(refused).expect_err("not a query");
        assert!(
            error.to_string().contains("--clock"),
            "{refused:?}: {error}"
        );
    }
    let unpinned = clock_for(&["dpm", "claim", "TEST-A", "--actor", "agent:a"]);
    assert_eq!(unpinned.expect("system clock"), QueryClock::System);
}
