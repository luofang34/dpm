use crate::EngineError;
use chrono::{DateTime, Utc};
use dpm_model::{EventTime, Plan, Timeline, WorkItem, WorkItemId, WorkKind, WorkStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Execution progress is distinct from independent acceptance of the result.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProgressSummary {
    /// Execution percentage; packages/workspace equally weight descendant leaf tasks.
    pub percent_complete: f64,
    /// Whether all applicable verification and aggregate gate conditions are satisfied.
    pub verified: bool,
    /// Verification time of a task, or the latest prerequisite or decision release of a milestone
    /// or package; unrecorded when any contributing event predates recorded event times.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<EventTime>,
}

/// Derived progress values, never persisted as replacement domain objects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressProjection {
    /// Equal-weight task progress for the workspace; milestones do not inflate the denominator.
    pub overall: ProgressSummary,
    /// Per-work task, container or milestone projection.
    pub work: BTreeMap<WorkItemId, ProgressSummary>,
}

/// Compute execution percentages without inferring work from elapsed time or changing the schedule.
///
/// `now` is the adapter's clock reading; it decides only whether lagged milestone gates are reached.
pub fn progress(plan: &Plan, now: DateTime<Utc>) -> Result<ProgressProjection, EngineError> {
    plan.validate()?;
    Ok(progress_with(plan, &Timeline::at(plan, now)))
}

pub(crate) fn progress_with(plan: &Plan, timeline: &Timeline) -> ProgressProjection {
    let done = timeline.completed();
    let tasks: Vec<_> = plan
        .work_items
        .values()
        .filter(|w| w.is_executable())
        .collect();
    let mut work = BTreeMap::new();
    for item in plan.work_items.values() {
        let verified = done.contains(&item.id);
        let percent_complete = match item.kind {
            WorkKind::Task => task_percent(item),
            WorkKind::Milestone => {
                if verified {
                    100.0
                } else {
                    0.0
                }
            }
            WorkKind::WorkPackage => {
                let descendants: Vec<_> = tasks
                    .iter()
                    .copied()
                    .filter(|t| within(plan, t, item.id))
                    .collect();
                average(&descendants, verified)
            }
        };
        work.insert(
            item.id,
            ProgressSummary {
                percent_complete,
                verified,
                completed_at: timeline.completed_at(item.id),
            },
        );
    }
    let verified = !plan.work_items.is_empty() && done.len() == plan.work_items.len();
    let completed_at = verified
        .then(|| {
            plan.work_items
                .keys()
                .filter_map(|id| timeline.completed_at(*id))
                .reduce(EventTime::latest)
        })
        .flatten();
    ProgressProjection {
        overall: ProgressSummary {
            percent_complete: average(&tasks, verified),
            verified,
            completed_at,
        },
        work,
    }
}

fn task_percent(work: &WorkItem) -> f64 {
    if matches!(
        work.status,
        WorkStatus::Submitted | WorkStatus::Verified | WorkStatus::Done
    ) {
        100.0
    } else {
        f64::from(work.reported_progress_percent)
    }
}

fn average(tasks: &[&WorkItem], verified: bool) -> f64 {
    if tasks.is_empty() {
        return if verified { 100.0 } else { 0.0 };
    }
    tasks
        .iter()
        .map(|w| task_percent(w) / tasks.len() as f64)
        .sum::<f64>()
        .clamp(0.0, 100.0)
}

fn within(plan: &Plan, task: &WorkItem, container: WorkItemId) -> bool {
    let mut parent = task.parent;
    let mut seen = BTreeSet::new();
    while let Some(id) = parent {
        if id == container {
            return true;
        }
        if !seen.insert(id) {
            return false;
        }
        parent = plan.work_items.get(&id).and_then(|w| w.parent);
    }
    false
}

#[cfg(test)]
mod tests;
