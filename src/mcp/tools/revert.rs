//! Reverting a recorded plan change: a read-only review of its inverse, then the revert itself,
//! which a human or service applies with a reason like any reviewed change.

use dpm_app::{AppError, Application, CommandRequest, Envelope, Query};
use dpm_model::{ActorId, OperationId};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Tool names this module owns, with the descriptions listed by `tools/list`.
pub(super) const NAMES: [(&str, &str); 2] = [
    (
        "propose_revert",
        "Review the inverse of a recorded plan change against the current plan without changing state; later changes or protected execution are refused with the entities named",
    ),
    (
        "revert_change",
        "Human or service applies the reviewed inverse of a recorded plan change with a reason, as a new operation naming the reverted one; history is never rewritten",
    ),
];

pub(super) fn handles(name: &str) -> bool {
    NAMES.iter().any(|(tool, _)| *tool == name)
}

pub(super) fn schema(name: &str, properties: &mut Map<String, Value>, needed: &mut Vec<&str>) {
    properties.insert(
        "operation".into(),
        json!({"type":"string","description":"Identity of the recorded plan change, as history reports it"}),
    );
    needed.push("operation");
    if name == "revert_change" {
        properties.insert("reason".into(), json!({"type":"string","minLength":1}));
        needed.push("reason");
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevertArguments {
    operation: Option<OperationId>,
    reason: Option<String>,
    base_revision: Option<u64>,
}

pub(super) fn call_blocking(
    app: &mut Application,
    actor: &ActorId,
    name: &str,
    value: Value,
    preconditions: super::preconditions::Preconditions,
) -> Result<Value, AppError> {
    let args: RevertArguments = serde_json::from_value(value)?;
    let operation = super::required(args.operation, "operation")?;
    if name == "propose_revert" {
        return Ok(serde_json::to_value(Envelope::from(
            app.query_blocking(Query::ProposeRevert { operation })?,
        ))?);
    }
    let base_revision = super::required(args.base_revision, "base_revision")?;
    let reason = super::required(args.reason, "reason")?;
    let command = app.build_command_blocking(preconditions.operation_id, |app| {
        app.revert_command_blocking(operation, reason)
    })?;
    let recorded = app.execute_blocking(CommandRequest {
        actor: actor.clone(),
        base_revision,
        base_lineage: preconditions.base_lineage,
        operation_id: preconditions.operation_id,
        command,
    })?;
    Ok(serde_json::to_value(Envelope::from(recorded))?)
}
