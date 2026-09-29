use super::*;
use serde_json::json;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture JSON")
}

#[test]
fn envelope_names_version_revision_and_data_and_keeps_an_absent_revision_explicit() {
    let observed = serde_json::to_value(Envelope::new(Some(4), json!({"x": 1}))).expect("json");
    assert_eq!(
        observed,
        json!({"api_version": API_VERSION, "revision": 4, "data": {"x": 1}})
    );
    let local = serde_json::to_value(Envelope::new(None, json!([]))).expect("json");
    assert_eq!(
        local,
        json!({"api_version": API_VERSION, "revision": null, "data": []})
    );
}

#[test]
fn a_query_response_keeps_its_revision_in_the_envelope() {
    let response = QueryResponse {
        api_version: API_VERSION,
        revision: 7,
        data: json!("view"),
    };
    assert_eq!(
        Envelope::from(response),
        Envelope::new(Some(7), json!("view"))
    );
}

#[test]
fn a_valid_plan_reports_its_identity_and_document_revision() {
    let plan = fixture();
    let report = validate_plan(plan.clone()).expect("valid fixture");
    assert!(report.valid);
    assert_eq!(report.format_version, 3);
    assert_eq!(json!(report.plan_revision), plan["revision"]);
    assert_eq!(json!(report.workspace), plan["workspace"]);
}

#[test]
fn undecodable_and_invariant_breaking_plans_are_distinct_refusals() {
    let mut future = fixture();
    future["format_version"] = json!(4);
    let error = validate_plan(future).expect_err("unsupported format");
    assert_eq!(error.code(), "invalid_request");
    assert!(
        error.to_string().contains("unsupported plan format 4"),
        "{error}"
    );

    let mut dangling = fixture();
    let edges = dangling["dependencies"].as_array_mut().expect("edges");
    edges[0]["predecessor"] = json!("00000000-0000-4000-8000-000000000000");
    let error = validate_plan(dangling).expect_err("dangling edge");
    assert_eq!(error.code(), "invalid_plan");
}
