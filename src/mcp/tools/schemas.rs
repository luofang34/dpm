//! Input schemas shared by several tools.

use dpm_app::AppError;
use serde_json::{Value, json};

/// An `Artifact` as `add_artifact` accepts it.
pub(super) fn artifact() -> Value {
    json!({"type":"object","required":["id","kind","uri","label","metadata","created_by","created_at"],"additionalProperties":false,
    "properties":{
        "id":{"type":"string","format":"uuid"},
        "kind":{"type":"string","enum":["GitCommit","PullRequest","File","Build","TestResult","Datasheet","Quote","PurchaseOrder","Cad","Photo","Other"]},
        "uri":{"type":"string","minLength":1},"label":{"type":"string","minLength":1},
        "metadata":{"type":"object","additionalProperties":{"type":"string"}},
        "created_by":{"type":"object","required":["kind","name"],"additionalProperties":false,
            "properties":{"kind":{"type":"string","enum":["Human","Agent","Service"]},"name":{"type":"string","minLength":1}}},
        "created_at":{"type":"string","format":"date-time"}
    }})
}

pub(super) fn identity() -> Value {
    let other = json!({"type":"object","required":["Other"],"additionalProperties":false,"properties":{"Other":{"type":"string","minLength":1}}});
    json!({"type":"object","required":["provider","instance","kind","external_id"],"additionalProperties":false,
    "description":"Provider-scoped identity, separate from label and URL; equal IDs on other instances or namespaces are different objects",
    "properties":{
        "provider":{"oneOf":[{"type":"string","enum":["GitHub","GitLab","Forgejo","Gitea","Jira","Linear"]},other]},
        "instance":{"type":"string","minLength":1,"description":"host[:port] of the hosted or self-hosted instance"},
        "namespace":{"type":"string","description":"Tenant, owner/repository or project namespace; required for forges and Linear"},
        "kind":{"oneOf":[{"type":"string","enum":["Issue","PullRequest"]},other],"description":"Kind from the provider table; kinds sharing a number space (GitHub issue/pull request/discussion, GitLab issue/incident/task, any Jira or Linear issue type) name one object"},
        "external_id":{"type":"string","minLength":1,"description":"A number on GitHub, GitLab, Forgejo and Gitea; a PROJECT-N key on Jira and Linear"}
    }})
}

/// Refuse an argument the tool's input schema does not declare.
pub(super) fn validate_argument_names(name: &str, value: &Value) -> Result<(), AppError> {
    let definition = super::definitions()
        .into_iter()
        .find(|d| d.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| AppError::InvalidRequest(format!("unknown tool {name}")))?;
    let fields = value
        .as_object()
        .ok_or_else(|| AppError::InvalidRequest("arguments must be an object".into()))?;
    for key in fields.keys() {
        if definition
            .pointer("/inputSchema/properties")
            .and_then(|properties| properties.get(key))
            .is_none()
        {
            return Err(AppError::InvalidRequest(format!(
                "unexpected argument {key} for {name}"
            )));
        }
    }
    Ok(())
}
