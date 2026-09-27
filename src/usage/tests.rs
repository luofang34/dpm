use super::*;
use crate::args::Cli;
use clap::Parser;

fn refused(arguments: &[&str]) -> clap::Error {
    match Cli::try_parse_from(arguments) {
        Ok(cli) => panic!("{arguments:?} parsed as {cli:?}"),
        Err(error) => error,
    }
}

fn arguments(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[test]
fn json_usage_errors_use_the_machine_envelope_with_a_stable_code() {
    for (argv, fragment) in [
        (&["dpm", "--json", "claim", "TEST-A"][..], "--actor"),
        (&["dpm", "claim", "TEST-A", "--json"][..], "--actor"),
        (&["dpm", "--json", "--bogus", "status"][..], "--bogus"),
        (
            &["dpm", "--json", "--base-revision", "abc", "status"][..],
            "abc",
        ),
    ] {
        let error = refused(argv);
        assert!(json_requested(arguments(argv)), "{argv:?}");
        let value = envelope(&error, true).expect("machine envelope");
        let body = &value["error"];
        assert_eq!(body["code"], "invalid_request", "{argv:?}");
        assert_eq!(body["api_version"], dpm_app::API_VERSION);
        let message = body["message"].as_str().expect("message");
        assert!(message.contains(fragment), "{argv:?}: {message}");
        assert!(
            !message.contains('\u{1b}'),
            "no terminal styling: {message}"
        );
        assert_ne!(error.exit_code(), 0);
    }
}

#[test]
fn human_callers_and_requested_help_keep_clap_text() {
    let error = refused(&["dpm", "claim", "TEST-A"]);
    assert!(!json_requested(arguments(&["dpm", "claim", "TEST-A"])));
    assert!(envelope(&error, false).is_none());
    let help = refused(&["dpm", "--json", "--help"]);
    assert!(envelope(&help, true).is_none());
    assert_eq!(help.exit_code(), 0);
    assert!(!json_requested(arguments(&[
        "dpm", "block", "TEST-A", "--", "--json"
    ])));
}
