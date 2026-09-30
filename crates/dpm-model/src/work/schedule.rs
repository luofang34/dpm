//! Authoritative work scheduling inputs.

use crate::*;
use serde::{Deserialize, Serialize};

/// Priority and uncertainty inputs used to derive schedule projections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleInputs {
    /// Human-assigned importance.
    pub priority: Priority,
    /// Optional duration uncertainty; absent estimates contribute zero hours. With calendars the
    /// hours are working hours of the task's calendar.
    pub estimate: Option<ThreePointEstimate>,
    /// Kind of actor expected to do the work while it has no owner; the workspace default when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<ActorKind>,
    /// Calendar this task follows regardless of who does it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<String>,
}
