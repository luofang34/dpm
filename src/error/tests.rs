use super::*;

#[test]
fn machine_errors_carry_the_api_version_that_agent_tool_errors_carry() {
    let error = CliError::from(dpm_app::AppError::UnknownWork("MISSING".into()));
    let expected =
        serde_json::to_value(dpm_app::AppError::UnknownWork("MISSING".into()).response())
            .expect("response");
    let envelope = error.envelope();
    assert_eq!(envelope["error"], expected);
    assert_eq!(envelope["error"]["api_version"], dpm_app::API_VERSION);
    assert_eq!(envelope["error"]["code"], "not_found");
    assert!(envelope["error"].get("details").is_none());
}
