use super::*;

fn parse(arguments: &[&str]) -> Result<Parsed, OptionsError> {
    Options::parse(arguments.iter().map(ToString::to_string))
}

fn run(arguments: &[&str]) -> Options {
    match parse(arguments).expect("valid options") {
        Parsed::Run(options) => options,
        Parsed::Help => panic!("expected options, not help"),
    }
}

#[test]
fn no_arguments_means_discovery_with_the_default_bounds() {
    let options = run(&[]);
    assert_eq!(options.project, None);
    assert_eq!(options.database, None);
    assert_eq!(options.limits, Limits::default());
    assert!(!options.verbose && options.clock.is_none());
}

#[test]
fn an_explicit_source_clock_and_bounds_are_read() {
    let options = run(&[
        "--project",
        "/work/plan",
        "--clock",
        "2026-10-02T12:00:00Z",
        "--max-request-bytes",
        "8192",
        "--max-response-bytes",
        "65536",
        "--verbose",
    ]);
    assert_eq!(options.project, Some("/work/plan".into()));
    assert_eq!(options.clock, "2026-10-02T12:00:00Z".parse().ok());
    assert_eq!(options.limits.max_request_bytes(), 8192);
    assert_eq!(options.limits.max_response_bytes(), 65536);
    assert!(options.verbose);
    assert_eq!(
        run(&["--database", "/work/state.sqlite"]).database,
        Some("/work/state.sqlite".into())
    );
}

#[test]
fn a_source_cannot_be_both_a_project_and_a_database() {
    assert!(matches!(
        parse(&["--project", "a", "--database", "b"]),
        Err(OptionsError::ConflictingSource)
    ));
}

#[test]
fn malformed_command_lines_are_refused_with_the_reason() {
    assert!(matches!(parse(&["--bogus"]), Err(OptionsError::Usage(_))));
    assert!(matches!(parse(&["--project"]), Err(OptionsError::Usage(_))));
    let clock = parse(&["--clock", "yesterday"]).expect_err("not a time");
    assert!(matches!(&clock, OptionsError::Clock { value, .. } if value == "yesterday"));
    assert!(
        std::error::Error::source(&clock).is_some(),
        "the parse failure is kept"
    );
    for value in ["-1", "many", "1.5"] {
        assert!(
            matches!(
                parse(&["--max-response-bytes", value]),
                Err(OptionsError::Bound { .. })
            ),
            "{value}"
        );
    }
    for value in ["12", "0", "1023", "999999999999"] {
        assert!(
            matches!(
                parse(&["--max-response-bytes", value]),
                Err(OptionsError::Limit { .. })
            ),
            "{value}"
        );
        assert!(
            matches!(
                parse(&["--max-request-bytes", value]),
                Err(OptionsError::Limit { .. })
            ),
            "{value}"
        );
    }
    assert!(matches!(parse(&["--help"]), Ok(Parsed::Help)));
}
