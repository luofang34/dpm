//! The success shape every adapter emits, and plan validation that needs no workspace.

use crate::{API_VERSION, AppError, QueryResponse};
use dpm_model::{LineageId, Plan, Workspace};
use dpm_store::RecordedOperation;
use serde::Serialize;
use serde_json::Value;

/// Success result of every CLI `--json` command and every agent tool call.
///
/// Both adapters build it here so a client parses one shape whichever process it talks to.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Envelope<T> {
    /// Wire contract version; see [`API_VERSION`].
    pub api_version: u32,
    /// Workspace revision the result observed or produced; `None` (JSON `null`) when the result
    /// reads no workspace, such as plan validation or device-local bindings.
    pub revision: Option<u64>,
    /// Writable history the revision belongs to; `None` (JSON `null`) when the result reads no
    /// store, such as a preview, plan validation or device-local bindings. A client that caches a
    /// revision caches this lineage with it and passes both back as preconditions.
    pub lineage_id: Option<LineageId>,
    /// Command-specific result.
    pub data: T,
}

impl<T> Envelope<T> {
    /// Wrap a result that observed `revision`, or none.
    pub fn new(revision: Option<u64>, data: T) -> Self {
        Self {
            api_version: API_VERSION,
            revision,
            lineage_id: None,
            data,
        }
    }
}

impl From<QueryResponse> for Envelope<Value> {
    fn from(response: QueryResponse) -> Self {
        Self {
            api_version: response.api_version,
            revision: Some(response.revision),
            lineage_id: response.lineage_id,
            data: response.data,
        }
    }
}

impl From<RecordedOperation> for Envelope<RecordedOperation> {
    fn from(operation: RecordedOperation) -> Self {
        Self {
            api_version: API_VERSION,
            revision: Some(operation.operation.resulting_revision),
            lineage_id: Some(operation.lineage_id),
            data: operation,
        }
    }
}

/// Summary of a plan that decoded and passed every graph invariant.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanValidation {
    /// Always true; an invalid plan is reported as an error instead.
    pub valid: bool,
    /// Portable plan format the document declares.
    pub format_version: u32,
    /// Workspace identity the plan belongs to.
    pub workspace: Workspace,
    /// Revision recorded in the document, not the revision of any open workspace.
    pub plan_revision: u64,
}

/// Decode and validate a portable plan that arrives as a JSON value (a tool argument).
pub fn validate_plan(value: Value) -> Result<PlanValidation, AppError> {
    validate_decoded(serde_json::from_value(value)?)
}

/// Validate a decoded plan as `import` would, without changing any state.
///
/// Callers holding plan text decode it with the same reader `import` uses, so text-only faults such
/// as a repeated field are refused here too.
pub fn validate_decoded(plan: Plan) -> Result<PlanValidation, AppError> {
    plan.validate()?;
    Ok(PlanValidation {
        valid: true,
        format_version: plan.format_version,
        workspace: plan.workspace,
        plan_revision: plan.revision,
    })
}

#[cfg(test)]
mod tests;
