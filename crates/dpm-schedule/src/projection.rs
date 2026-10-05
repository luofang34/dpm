use dpm_model::WorkItemId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Serialize, Deserialize)]
/// Earliest/latest times and float for one activity, in elapsed hours after the origin.
pub struct ActivitySchedule {
    /// Minimum feasible start relative to the projection origin.
    pub earliest_start_hours: f64,
    /// Earliest start plus activity duration.
    pub earliest_finish_hours: f64,
    /// Latest start that does not delay project completion.
    pub latest_start_hours: f64,
    /// Latest start plus activity duration.
    pub latest_finish_hours: f64,
    /// Delay available without postponing project completion.
    pub total_float_hours: f64,
    /// Delay available without moving any direct successor's earliest start or project completion.
    pub free_float_hours: f64,
    /// Whether total float is within numerical tolerance of zero.
    pub critical: bool,
    /// Calendar the task follows and how long its verification waits for the verifier, when the
    /// plan has calendars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<ActivityCalendar>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// How calendars place one task in a remaining projection.
pub struct ActivityCalendar {
    /// Calendar, executor kind and the rule that chose them.
    #[serde(flatten)]
    pub resolved: dpm_model::ResolvedCalendar,
    /// Hours between the end of execution and verification at the earliest times, waiting for
    /// the verifier's calendar.
    pub review_wait_hours: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Derived critical-path projection for the entire graph.
pub struct Schedule {
    /// Earliest possible completion of every activity.
    pub project_finish_hours: f64,
    /// Activity projections indexed by work identity.
    pub activities: BTreeMap<WorkItemId, ActivitySchedule>,
    /// Work identities with zero total float.
    pub critical_activities: Vec<WorkItemId>,
    /// Every constraint between scheduled activities, with its slack at the earliest times.
    #[serde(default)]
    pub relations: BTreeMap<dpm_model::DependencyId, RelationSchedule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
/// How tightly one constraint binds its successor in the projection.
pub struct RelationSchedule {
    /// How far the successor's constrained event lies past the bound this constraint puts on it.
    pub slack_hours: f64,
    /// The constraint sets its successor's earliest start, or on calendars holds its finish: its
    /// slack is within tolerance of zero.
    pub driving: bool,
    /// A driving constraint between two critical activities, so it lies on a critical path.
    pub critical: bool,
}
