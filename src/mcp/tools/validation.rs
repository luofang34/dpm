//! Plan validation and the plan schema, which read no workspace, so they answer the same on a
//! preview or a live store and report no revision, like their CLI commands.

use super::required;
use dpm_app::{AppError, Envelope};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Tool names this module owns, with the descriptions listed by `tools/list`.
pub(super) const NAMES: [(&str, &str); 1] = [(
    "validate_plan",
    "Check that a portable plan decodes and satisfies every graph invariant, exactly as CLI validate and import do; changes nothing and needs no workspace",
)];

pub(super) fn handles(name: &str) -> bool {
    NAMES.iter().any(|(tool, _)| *tool == name)
}

pub(super) fn schema(properties: &mut Map<String, Value>, needed: &mut Vec<&str>) {
    properties.insert(
        "plan".into(),
        json!({"type":"object","description":"Complete portable plan (shape: plan_schema), such as an export_plan result or a file prepared for CLI import"}),
    );
    needed.push("plan");
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidationArguments {
    plan: Option<Value>,
}

/// The plan stays an undecoded value until validation so decoding errors read as CLI validate's.
pub(super) fn call(name: &str, value: Value) -> Result<Value, AppError> {
    if name == "plan_schema" {
        return Ok(serde_json::to_value(Envelope::new(
            None,
            dpm_app::plan_schema()?,
        ))?);
    }
    let args: ValidationArguments = serde_json::from_value(value)?;
    let report = dpm_app::validate_plan(required(args.plan, "plan")?)?;
    Ok(serde_json::to_value(Envelope::new(None, report))?)
}
