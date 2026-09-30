//! Actual execution time of verified tasks against their estimates.

use super::history::occurred;
use super::{
    EstimateCalibration, EstimateSample, ExclusionReason, HistoryIndex, MIN_ACTUAL_SECONDS,
    exclusions,
};
use crate::{Command, Operation};
use chrono::{DateTime, Utc};
use dpm_model::{ActorKind, AttemptOutcome, Key, Plan, WorkItem, WorkItemId};
use std::collections::BTreeSet;

mod attempt;
mod bulk;
mod groups;

/// Estimate calibration and the tasks whose start and submit were recorded in bulk.
pub(super) struct Measured {
    pub(super) calibration: EstimateCalibration,
    pub(super) bulk: BTreeSet<WorkItemId>,
}

/// A verified task left out of the samples, attributed to an executor kind when one is known.
pub(super) struct Excluded<'a> {
    kind: Option<ActorKind>,
    capabilities: &'a BTreeSet<String>,
    reason: ExclusionReason,
}

/// Measure every verified task, or say why it cannot be measured.
pub(super) fn measure(plan: &Plan, index: &HistoryIndex<'_>) -> Measured {
    let bulk: BTreeSet<WorkItemId> = plan
        .work_items
        .values()
        .filter(|w| w.is_executable() && bulk::bulk_recorded(index, w.id))
        .map(|w| w.id)
        .collect();
    let mut verified: Vec<&WorkItem> = plan
        .work_items
        .values()
        .filter(|w| w.is_executable() && w.execution.status.satisfies_dependency())
        .collect();
    verified.sort_by(|a, b| a.key.natural_cmp(&b.key));
    let mut samples = Vec::new();
    let mut excluded: Vec<Excluded<'_>> = Vec::new();
    let mut keys: Vec<(ExclusionReason, Key)> = Vec::new();
    for work in verified {
        match sample(plan, index, &bulk, work) {
            Ok(sample) => samples.push(sample),
            Err(reason) => {
                keys.push((reason, work.key.clone()));
                excluded.push(Excluded {
                    kind: attributed(index, work),
                    capabilities: &work.contract.capabilities,
                    reason,
                });
            }
        }
    }
    Measured {
        calibration: EstimateCalibration {
            by_executor: groups::groups(&samples, &excluded, false),
            by_capability: groups::groups(&samples, &excluded, true),
            samples,
            excluded: exclusions(keys),
        },
        bulk,
    }
}

/// One verified task's sample, or the first reason it cannot be one.
fn sample(
    plan: &Plan,
    index: &HistoryIndex<'_>,
    bulk: &BTreeSet<WorkItemId>,
    work: &WorkItem,
) -> Result<EstimateSample, ExclusionReason> {
    let estimated = work.expected_duration_hours();
    let estimated = (estimated > 0.0)
        .then_some(estimated)
        .ok_or(ExclusionReason::Unestimated)?;
    let started = work
        .execution
        .events
        .started_at
        .ok_or(ExclusionReason::StartUnrecorded)?;
    let submitted = verified_submission(work).ok_or(ExclusionReason::SubmitUnrecorded)?;
    let start_op = index.last(work.id, |c| matches!(c, Command::Start { .. }));
    let (Some(start_op), Some(submit_op)) = (start_op, submit_operation(index, work.id, submitted))
    else {
        return Err(ExclusionReason::NotInHistory);
    };
    if bulk.contains(&work.id) {
        return Err(ExclusionReason::BulkRecorded);
    }
    if attempt::mixed_executors(index.of(work.id), start_op, submit_op) {
        return Err(ExclusionReason::MixedExecutors);
    }
    let actual = attempt::actual_hours(plan, work, started, submitted)?;
    if actual < MIN_ACTUAL_SECONDS as f64 / 3600.0 {
        return Err(ExclusionReason::NoWorkingTime);
    }
    Ok(EstimateSample {
        key: work.key.clone(),
        executor: submit_op.actor.kind,
        capabilities: work.contract.capabilities.clone(),
        estimated_hours: estimated,
        actual_hours: actual,
        ratio: actual / estimated,
    })
}

/// Submission time of the attempt that was verified; the recorded submission for work verified
/// before attempts were recorded.
fn verified_submission(work: &WorkItem) -> Option<DateTime<Utc>> {
    match work.execution.attempts.last() {
        Some(attempt) => matches!(attempt.outcome, AttemptOutcome::Verified { .. })
            .then_some(attempt.submitted_at),
        None => work.execution.events.submitted_at,
    }
}

/// The logged submit that recorded the submission at `submitted`.
fn submit_operation<'a>(
    index: &HistoryIndex<'a>,
    work: WorkItemId,
    submitted: DateTime<Utc>,
) -> Option<&'a Operation> {
    index
        .of(work)
        .iter()
        .rev()
        .find(|op| matches!(op.command, Command::Submit { .. }) && occurred(op) == submitted)
        .copied()
}

/// Kind an excluded task's time would have counted for: its verified submitter's, else its latest
/// submitter's, else its owner's.
fn attributed(index: &HistoryIndex<'_>, work: &WorkItem) -> Option<ActorKind> {
    let submitter = verified_submission(work)
        .and_then(|at| submit_operation(index, work.id, at))
        .or_else(|| index.last(work.id, |c| matches!(c, Command::Submit { .. })));
    submitter
        .map(|op| op.actor.kind)
        .or_else(|| work.execution.owner.as_ref().map(|o| o.kind))
}
