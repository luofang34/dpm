//! Operation identity and lineage precondition accepted by every mutation tool. They are taken out
//! of the arguments before a tool parses its own, so each tool's argument struct stays unchanged.

use dpm_app::AppError;
use dpm_model::{LineageId, OperationId};
use serde_json::{Map, Value, json};

/// The mutation arguments shared by every tool that takes `base_revision`.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Preconditions {
    pub(super) operation_id: Option<OperationId>,
    pub(super) base_lineage: Option<LineageId>,
}

pub(super) fn schema(properties: &mut Map<String, Value>) {
    properties.insert(
        "operation_id".into(),
        json!({"type":"string","format":"uuid","description":"Version 7 UUID identifying this operation and its idempotency key; resending it with the same content returns the recorded operation, other content is refused with duplicate_operation. Minted when omitted"}),
    );
    properties.insert(
        "base_lineage".into(),
        json!({"type":"string","format":"uuid","description":"lineage_id observed with base_revision; a store continuing another lineage (such as a restored copy) refuses the operation with lineage_mismatch"}),
    );
}

/// Remove and parse the shared arguments; absent ones stay `None`.
pub(super) fn take(value: &mut Value) -> Result<Preconditions, AppError> {
    let Some(fields) = value.as_object_mut() else {
        return Ok(Preconditions::default());
    };
    let mut parse = |name: &str| {
        fields
            .remove(name)
            .map(serde_json::from_value::<String>)
            .transpose()
            .map_err(AppError::from)
    };
    let (operation, lineage) = (parse("operation_id")?, parse("base_lineage")?);
    let invalid =
        |name: &str, text: &str| AppError::InvalidRequest(format!("{name} {text} is not a UUID"));
    Ok(Preconditions {
        operation_id: operation
            .map(|text| text.parse().map_err(|_| invalid("operation_id", &text)))
            .transpose()?,
        base_lineage: lineage
            .map(|text| text.parse().map_err(|_| invalid("base_lineage", &text)))
            .transpose()?,
    })
}
