use super::*;
use clap::CommandFactory;

#[test]
fn help_describes_every_option_and_the_actor_format() {
    let help = Launch::command().render_help().to_string();
    for fragment in [
        "--database <PATH>",
        "SQLite store to serve",
        "--project <DIR>",
        ".dpm/project.toml",
        "--actor <KIND:NAME>",
        "human:NAME, agent:NAME or service:NAME",
        "discovered from the current directory",
    ] {
        assert!(help.contains(fragment), "missing {fragment:?} in\n{help}");
    }
}

#[test]
fn actor_values_name_a_kind_and_a_nonempty_name() {
    let parsed = Launch::try_parse_from(["dpm-mcp", "--db", "x.sqlite", "--actor", "agent:coder"])
        .expect("arguments");
    assert_eq!(parsed.actor, ActorId::agent("coder"));
    assert_eq!(parsed.database, Some(PathBuf::from("x.sqlite")));
    for invalid in ["coder", "agent:", "robot:coder"] {
        assert!(parse_actor(invalid).is_err(), "{invalid}");
        assert!(Launch::try_parse_from(["dpm-mcp", "--actor", invalid]).is_err());
    }
}
