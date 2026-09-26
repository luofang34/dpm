use dpm_model::WorkItemId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Serialize, Deserialize)]
/// Earliest/latest times and float for one activity, in elapsed hours.
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
}
