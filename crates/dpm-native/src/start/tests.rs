use super::*;
use serde_json::Value;

fn options(project: Option<&std::path::Path>, database: Option<&std::path::Path>) -> Options {
    Options {
        project: project.map(Into::into),
        database: database.map(Into::into),
        ..Options::default()
    }
}

#[test]
fn a_missing_store_is_refused_with_the_shared_code_and_never_created() {
    let directory = tempfile::tempdir().expect("directory");
    let missing = directory.path().join("nowhere.sqlite");
    let Err(error) = start_blocking(&options(None, Some(&missing))) else {
        panic!("there is nothing to open");
    };
    assert!(matches!(error, StartError::Workspace(_)));
    assert!(
        !missing.exists(),
        "a missing workspace is never initialized"
    );
    // The frame carries the application's own code, not text a host would have to parse.
    let frame: Value = serde_json::from_str(&error.refusal_line(4096)).expect("one JSON frame");
    assert_eq!(frame["ok"], false);
    assert_eq!(frame["id"], "");
    assert_eq!(frame["error"]["code"], error.code());
    assert!(
        std::error::Error::source(&error).is_some(),
        "the cause is kept"
    );
}

#[test]
fn a_directory_with_no_locator_is_refused_without_falling_through() {
    let directory = tempfile::tempdir().expect("directory");
    let Err(error) = start_blocking(&options(Some(directory.path()), None)) else {
        panic!("there is no locator");
    };
    assert!(matches!(error, StartError::Workspace(_)), "{error}");
    let frame: Value = serde_json::from_str(&error.refusal_line(4096)).expect("frame");
    assert_eq!(frame["error"]["code"], error.code());
}

#[test]
fn a_command_line_failure_is_a_structured_refusal_with_its_cause() {
    let failure = Options::parse(["--clock".to_string(), "soon".to_string()]).expect_err("bad");
    let error = StartError::Options(failure);
    assert_eq!(error.code(), codes::INVALID_OPTIONS);
    assert!(std::error::Error::source(&error).is_some());
    let frame: Value = serde_json::from_str(&error.refusal_line(4096)).expect("frame");
    assert_eq!(frame["error"]["code"], "invalid_options");
}
