//! Namespaced custom data survives writers that replace task GUIDs or names.

use super::encoding::duration_seconds;
use super::source::{children, text};
use crate::InterchangeError;
use dpm_model::{Key, ThreePointEstimate, WorkItem, WorkItemId};
use roxmltree::Node;
use serde::{Deserialize, Serialize};

pub(super) const FIELD_ID: &str = "188743731";
pub(super) const ALIAS: &str = "DPM.Metadata.v1";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Metadata {
    version: u32,
    pub(crate) id: WorkItemId,
    pub(crate) key: Key,
    pub(crate) estimate: Option<ThreePointEstimate>,
}

pub(super) fn encode(work: &WorkItem) -> Result<String, String> {
    serde_json::to_string(&Metadata {
        version: 1,
        id: work.id,
        key: work.key.clone(),
        estimate: work.schedule.estimate,
    })
    .map_err(|error| format!("cannot encode DPM metadata: {error}"))
}

pub(super) fn field(root: Node) -> Result<Option<String>, InterchangeError> {
    let fields: std::collections::BTreeSet<_> = children(root, "ExtendedAttributes")
        .flat_map(|n| children(n, "ExtendedAttribute"))
        .filter(|n| text(*n, "Alias") == Some(ALIAS))
        .filter_map(|n| text(n, "FieldID").map(str::to_owned))
        .collect();
    if fields.len() > 1 {
        return Err(InterchangeError::MalformedProject {
            reason: "conflicting DPM metadata field definitions".into(),
        });
    }
    Ok(fields.into_iter().next())
}

pub(super) fn parse(
    node: Node,
    field: Option<&str>,
    position: usize,
) -> Result<Option<Metadata>, InterchangeError> {
    let Some(field) = field else {
        return Ok(None);
    };
    let values: Vec<_> = children(node, "ExtendedAttribute")
        .filter(|n| text(*n, "FieldID") == Some(field))
        .collect();
    let invalid = |reason: &str| InterchangeError::MalformedTask {
        position,
        reason: reason.into(),
    };
    if values.len() > 1 {
        return Err(invalid("duplicate DPM metadata on one task"));
    }
    let Some(node) = values.first() else {
        return Ok(None);
    };
    let metadata: Metadata = serde_json::from_str(text(*node, "Value").unwrap_or_default())
        .map_err(|source| InterchangeError::Metadata { position, source })?;
    if metadata.version != 1 || metadata.id.0.is_nil() || metadata.key.0.trim().is_empty() {
        return Err(invalid(
            "DPM metadata requires version 1, a non-nil ID and a nonempty key",
        ));
    }
    if metadata.estimate.is_some_and(|e| e.validate().is_err()) {
        return Err(invalid(
            "DPM metadata contains an invalid three-point estimate",
        ));
    }
    Ok(Some(metadata))
}

impl Metadata {
    pub(super) fn matches_duration(&self, seconds: u64) -> bool {
        duration_seconds(self.estimate) == seconds
    }
}
