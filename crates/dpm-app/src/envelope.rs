//! The success shape every adapter emits, and plan validation that needs no workspace.

use crate::{API_VERSION, AppError, QueryResponse};
use dpm_engine::Operation;
use dpm_model::{Plan, Workspace};
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
    /// Command-specific result.
    pub data: T,
}

impl<T> Envelope<T> {
    /// Wrap a result that observed `revision`, or none.
    pub fn new(revision: Option<u64>, data: T) -> Self {
        Self {
            api_version: API_VERSION,
            revision,
            data,
        }
    }
}

impl From<QueryResponse> for Envelope<Value> {
    fn from(response: QueryResponse) -> Self {
        Self {
            api_version: response.api_version,
            revision: Some(response.revision),
            data: response.data,
        }
    }
}

impl From<Operation> for Envelope<Operation> {
    fn from(operation: Operation) -> Self {
        Self::new(Some(operation.resulting_revision), operation)
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

/// Decode and validate a portable plan exactly as `import` would, without changing any state.
///
/// Decoding from a JSON value rather than text keeps the diagnostic identical whether the plan
/// arrives as a file or as a tool argument.
pub fn validate_plan(value: Value) -> Result<PlanValidation, AppError> {
    let plan: Plan = serde_json::from_value(value)?;
    plan.validate()?;
    Ok(PlanValidation {
        valid: true,
        format_version: plan.format_version,
        workspace: plan.workspace,
        plan_revision: plan.revision,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests;
