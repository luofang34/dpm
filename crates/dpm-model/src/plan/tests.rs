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
