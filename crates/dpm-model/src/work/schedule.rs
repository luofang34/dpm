//! Authoritative work scheduling inputs.

use crate::*;
use serde::{Deserialize, Serialize};

/// Priority and uncertainty inputs used to derive schedule projections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleInputs {
    /// Human-assigned importance.
    pub priority: Priority,
    /// Optional duration uncertainty; absent estimates contribute zero hours.
    pub estimate: Option<ThreePointEstimate>,
}
