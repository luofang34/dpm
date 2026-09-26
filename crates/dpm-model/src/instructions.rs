use serde::{Deserialize, Serialize};

/// Author-supplied procedure and scope for an executable task, not execution authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkInstructions {
    /// Ordered actions; their array order defines the suggested procedure.
    pub steps: Vec<ExecutionStep>,
    /// Deliverables and changes permitted by this contract.
    pub in_scope: Vec<String>,
    /// Explicit exclusions to prevent unapproved scope expansion.
    pub out_of_scope: Vec<String>,
    /// Checks and evidence to collect against the task's acceptance criteria.
    pub verification: Vec<String>,
}

/// One inspectable action and the observable result it should produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionStep {
    /// Concrete action for the worker.
    pub action: String,
    /// Observable output used to assess completion of this step.
    pub expected_result: String,
}
