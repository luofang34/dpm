use crate::{EngineError, Transition};
use chrono::{DateTime, Utc};
pub use dpm_model::completion;
use dpm_model::{Plan, Timeline, WorkItem, WorkItemId, WorkStatus};

/// Whether a planned task can be claimed at an adapter-supplied time.
///
/// Claim eligibility is the start gate: FS and SS predecessor events plus positive lag, and every
/// decision on the task or its containers. FF and SF constrain submission and verification instead.
pub fn is_ready(plan: &Plan, work: &WorkItem, now: DateTime<Utc>) -> bool {
    plan.work_items.get(&work.id) == Some(work)
        && crate::gate_report(plan, work.id, Transition::Claim, now)
            .is_ok_and(|report| report.ready)
}

/// Return a work projection with aggregate completion reflected in its lifecycle.
///
/// The returned value is a view, not an authoritative snapshot to persist.
pub fn show_work(
    plan: &Plan,
    work: WorkItemId,
    now: DateTime<Utc>,
) -> Result<WorkItem, EngineError> {
    plan.validate()?;
    let timeline = Timeline::at(plan, now);
    show_with(plan, work, &timeline)
}

pub(crate) fn show_with(
    plan: &Plan,
    work: WorkItemId,
    timeline: &Timeline,
) -> Result<WorkItem, EngineError> {
    let mut item = plan
        .work_items
        .get(&work)
        .ok_or(EngineError::MissingWorkItem(work))?
        .clone();
    if !item.is_executable() && timeline.completed_at(work).is_some() {
        item.execution.status = WorkStatus::Verified;
    }
    Ok(item)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
