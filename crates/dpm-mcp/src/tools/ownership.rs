//! Release and handoff tools. Their arguments are parsed on their own so the shared argument
//! struct stays within the member limit.

use dpm_app::{AppError, Application, CommandRequest};
use dpm_model::ActorId;
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Tool names this module owns, with the descriptions listed by `tools/list`.
pub(super) const NAMES: [(&str, &str); 2] = [
    (
        "release_work",
        "Give up this actor's own unstarted claim with a reason; the task returns to Planned without an owner. Started work cannot be released",
    ),
    (
        "handoff_work",
        "Human or service transfers claimed, started or blocked work to another actor (KIND:NAME) with a reason; events, attempts, basis, progress and blocker stay, and no former owner may later review the work",
    ),
];

pub(super) fn handles(name: &str) -> bool {
    NAMES.iter().any(|(tool, _)| *tool == name)
}

pub(super) fn schema(name: &str, properties: &mut Map<String, Value>, needed: &mut Vec<&str>) {
    properties.insert("key".into(), json!({"type":"string"}));
    properties.insert("reason".into(), json!({"type":"string","minLength":1}));
    needed.extend(["key", "reason"]);
    if name == "handoff_work" {
        properties.insert(
            "to".into(),
            json!({"type":"string","pattern":"^(human|agent|service):.+$","description":"New owner as KIND:NAME"}),
        );
        needed.push("to");
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnershipArguments {
    key: Option<String>,
    reason: Option<String>,
    to: Option<String>,
    base_revision: Option<u64>,
}

pub(super) fn call_blocking(
    app: &mut Application,
    actor: &ActorId,
    name: &str,
    value: Value,
) -> Result<Value, AppError> {
    let args: OwnershipArguments = serde_json::from_value(value)?;
    let base_revision = super::required(args.base_revision, "base_revision")?;
    app.ensure_writable()?;
    let key = super::required(args.key, "key")?;
    let reason = super::required(args.reason, "reason")?;
    let command = if name == "handoff_work" {
        app.handoff_command_blocking(&key, &super::required(args.to, "to")?, reason)?
    } else {
        app.release_command_blocking(&key, reason)?
    };
    let operation = app.execute_blocking(CommandRequest {
        actor: actor.clone(),
        base_revision,
        command,
    })?;
    Ok(serde_json::to_value(dpm_app::Envelope::from(operation))?)
}
