//! When a lifecycle event occurred, as distinct from when its operation commits.
//!
//! Work recorded after the fact keeps its honest time for forecasts and calibration, but a
//! supplied time can neither anticipate the commit nor reorder the task's recorded history.

use crate::{EngineError, Transition};
use chrono::{DateTime, Utc};
use dpm_model::{Plan, WorkItemId};

/// The event time a transition records: the supplied occurrence time, or the commit time.
pub(super) fn resolve(
    plan: &Plan,
    work: WorkItemId,
    transition: Transition,
    occurred_at: Option<DateTime<Utc>>,
    committed_at: DateTime<Utc>,
) -> Result<DateTime<Utc>, EngineError> {
    let Some(occurred_at) = occurred_at else {
        return Ok(committed_at);
    };
    let item = plan
        .work_items
        .get(&work)
        .ok_or(EngineError::MissingWorkItem(work))?;
    if occurred_at > committed_at {
        return Err(EngineError::OccurrenceInFuture {
            work,
            key: item.key.clone(),
            transition,
            occurred_at,
            committed_at,
        });
    }
    match item.execution.latest_recorded_at() {
        Some(previous_at) if occurred_at < previous_at => {
            Err(EngineError::OccurrenceBeforePrevious {
                work,
                key: item.key.clone(),
                transition,
                occurred_at,
                previous_at,
            })
        }
        _ => Ok(occurred_at),
    }
}
