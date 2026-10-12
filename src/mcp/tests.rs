use super::*;

#[test]
fn process_wide_preconditions_are_refused_and_none_is_accepted() {
    let none = Preconditions {
        base_revision: None,
        base_lineage: None,
        operation_id: None,
    };
    assert!(refuse_process_preconditions(none).is_ok());
    let revision = Preconditions {
        base_revision: Some(3),
        ..none
    };
    let refused = refuse_process_preconditions(revision).expect_err("refused");
    assert_eq!(refused.code(), "invalid_request");
    assert!(refused.to_string().contains("--base-revision"), "{refused}");
}

#[test]
fn a_startup_failure_leads_with_its_code_on_stderr_but_not_in_the_envelope() {
    let failure = CliError::McpStartup(Box::new(CliError::Input("actor must be named".into())));
    assert_eq!(failure.to_string(), "invalid_request: actor must be named");
    let envelope = failure.envelope();
    assert_eq!(envelope["error"]["code"], "invalid_request");
    assert_eq!(envelope["error"]["message"], "actor must be named");
}
