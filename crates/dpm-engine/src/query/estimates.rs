//! Work whose missing estimate the remaining forecast silently counts as zero hours.
//!
//! The remaining CPM and Monte Carlo keep a task without an estimate at 0 h rather than guessing a
//! duration, so a forecast over such work is optimistic. These projections name that work so a
//! reader can judge the forecast; they never change its arithmetic.

use dpm_model::{Key, Plan, Timeline, WorkItem};

/// Whether `work` enters the remaining forecast at 0 h only because it has no estimate.
///
/// It follows the remaining projection, which a test checks: a task (milestones and packages have no
/// duration of their own) in the active graph (applicable, so not excluded, undecided, awaiting a
/// choice, stranded or an empty join) that is not yet verified or done (a submitted task still
/// keeps its full duration until independent verification).
#[must_use]
pub fn is_unestimated(timeline: &Timeline, work: &WorkItem) -> bool {
    work.is_executable()
        && work.schedule.estimate.is_none()
        && !work.execution.status.satisfies_dependency()
        && timeline.applicability(work.id).is_applicable()
}

/// Keys of every work item [`is_unestimated`] reports, in key order.
#[must_use]
pub fn unestimated(plan: &Plan, timeline: &Timeline) -> Vec<Key> {
    let mut keys: Vec<_> = plan
        .work_items
        .values()
        .filter(|w| is_unestimated(timeline, w))
        .map(|w| w.key.clone())
        .collect();
    keys.sort_by(dpm_model::Key::natural_cmp);
    keys
}

#[cfg(test)]
mod tests;
