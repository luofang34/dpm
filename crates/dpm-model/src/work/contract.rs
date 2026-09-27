//! Authored work scope and readiness requirements.

use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The authored scope an executor must satisfy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkContract {
    /// Observable purpose of this work.
    pub objective: String,
    /// Evidence requirements for executable work.
    pub acceptance: Vec<AcceptanceCriterion>,
    /// Optional ordered procedure and scope; absent in plans that do not supply instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<crate::WorkInstructions>,
    /// Capabilities required of a recommended worker.
    pub capabilities: BTreeSet<String>,
    /// Requirements implemented by this work.
    pub requirement_ids: BTreeSet<RequirementId>,
    /// Explicit read/write needs; an empty set permits work without repository assets.
    pub assets: Vec<crate::AssetRequirement>,
    /// Decision option this work and its descendants apply to; absent means unconditional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<crate::WorkCondition>,
    /// How a task or milestone treats incoming constraints from work a choice excluded.
    #[serde(default, skip_serializing_if = "crate::JoinPolicy::is_default")]
    pub join: crate::JoinPolicy,
}
