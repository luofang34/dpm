use super::*;

#[test]
fn unsupported_version_names_the_preserving_conversion_path() {
    for version in [0, 1, 2, 4, u32::MAX] {
        let input = format!("{{\"format_version\":{version}}}");
        let error = serde_json::from_str::<Plan>(&input)
            .expect_err("unsupported")
            .to_string();
        assert!(error.contains(&format!("unsupported plan format {version}")));
        assert!(error.contains("Preserve the original database"));
    }
}

#[test]
fn version_precedes_shape_errors_for_every_document_field_order() {
    for version in [0, 1, 2, 4, u32::MAX] {
        for body in [
            r#""resources":{}"#,
            r#""work_items":{"not-a-uuid":{"objective":"flat work"}}"#,
            r#""future_field":{"nested":[true,0,null]}"#,
        ] {
            for input in [
                format!("{{{body},\"format_version\":{version}}}"),
                format!("{{\"format_version\":{version},{body}}}"),
            ] {
                let error = serde_json::from_str::<Plan>(&input)
                    .expect_err("unsupported")
                    .to_string();
                assert!(
                    error.contains(&format!("unsupported plan format {version}")),
                    "{error}"
                );
                assert!(error.contains("Preserve the original database"), "{error}");
            }
        }
    }
}

#[test]
fn version_buffer_keeps_duplicate_fields_strict_and_full_revision_precision() {
    let mut plan = Plan::empty("portable");
    plan.revision = u64::MAX;
    let json = serde_json::to_string(&plan).expect("json");
    assert_eq!(serde_json::from_str::<Plan>(&json).expect("decode"), plan);
    for (input, field) in [
        (
            json.replacen("\"revision\":", "\"revision\":0,\"revision\":", 1),
            "revision",
        ),
        (
            json.replacen("\"name\":", "\"name\":\"extra\",\"name\":", 1),
            "name",
        ),
    ] {
        let error = serde_json::from_str::<Plan>(&input)
            .expect_err("duplicate")
            .to_string();
        assert!(
            error.contains(&format!("duplicate field `{field}`")),
            "{error}"
        );
    }
}

#[test]
fn buffered_plan_refuses_unconsumed_enum_and_sequence_entries() {
    let original = include_str!("../../../../tests/support/execution-plan.json");
    let plan: serde_json::Value = serde_json::from_str(original).expect("fixture");
    let asset = plan["assets"]
        .as_object()
        .expect("assets")
        .keys()
        .next()
        .expect("asset")
        .clone();
    let task = plan["work_items"]
        .as_object()
        .expect("work")
        .iter()
        .find(|(_, w)| w["kind"] == "Task")
        .expect("task")
        .0
        .clone();
    for estimate in [serde_json::json!([2, 4, 6, 999]), serde_json::json!([2, 4])] {
        let mut invalid = plan.clone();
        invalid["work_items"][&task]["schedule"]["estimate"] = estimate;
        assert!(serde_json::from_value::<Plan>(invalid).is_err());
    }
    let mut invalid = plan.clone();
    invalid["assets"][&asset]["kind"] =
        serde_json::json!({"GitRepository": {"remotes": []}, "Other": "must not disappear"});
    assert!(serde_json::from_value::<Plan>(invalid).is_err());
    let json = serde_json::to_string(&plan).expect("json");
    let duplicate = json.replace(
        r#""kind":{"GitRepository":{"remotes":[]}}"#,
        r#""kind":{"GitRepository":{"remotes":[]},"GitRepository":{"remotes":[]}}"#,
    );
    assert_ne!(duplicate, json);
    assert!(serde_json::from_str::<Plan>(&duplicate).is_err());
}
