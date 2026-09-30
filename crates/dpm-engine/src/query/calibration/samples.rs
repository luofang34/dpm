//! Actual execution time of verified tasks against their estimates.

use super::history::{backfilled, occurred};
use super::{
    BULK_WINDOW_SECONDS, EstimateCalibration, EstimateSample, ExclusionReason, HistoryIndex,
    MIN_SAMPLES, RatioGroup, ascending, exclusions,
};
use crate::{Command, EngineError};
use chrono::{DateTime, Utc};
use dpm_model::{ActorKind, AttemptOutcome, Key, Plan, WorkItem, WorkItemId};
use dpm_schedule::percentile;
use std::collections::{BTreeMap, BTreeSet};

/// Estimate calibration and the tasks whose start and submit were recorded in bulk.
pub(super) struct Measured {
    pub(super) calibration: EstimateCalibration,
    pub(super) bulk: BTreeSet<WorkItemId>,
}

/// Measure every verified task, or say why it cannot be measured.
pub(super) fn measure(plan: &Plan, index: &HistoryIndex<'_>) -> Result<Measured, EngineError> {
    let bulk: BTreeSet<WorkItemId> = plan
        .work_items
        .values()
        .filter(|w| w.is_executable() && bulk_recorded(index, w.id))
        .map(|w| w.id)
        .collect();
    let mut verified: Vec<&WorkItem> = plan
        .work_items
        .values()
        .filter(|w| w.is_executable() && w.execution.status.satisfies_dependency())
        .collect();
    verified.sort_by(|a, b| a.key.natural_cmp(&b.key));
    let mut samples = Vec::new();
    let mut excluded: Vec<(ExclusionReason, Key)> = Vec::new();
    for work in verified {
        match sample(plan, index, &bulk, work)? {
            Ok(sample) => samples.push(sample),
            Err(reason) => excluded.push((reason, work.key.clone())),
        }
    }
    Ok(Measured {
        calibration: EstimateCalibration {
            by_executor: groups(&samples, false),
            by_capability: groups(&samples, true),
            samples,
            excluded: exclusions(excluded),
        },
        bulk,
    })
}

/// One verified task's sample, or the first reason it cannot be one.
fn sample(
    plan: &Plan,
    index: &HistoryIndex<'_>,
    bulk: &BTreeSet<WorkItemId>,
    work: &WorkItem,
) -> Result<Result<EstimateSample, ExclusionReason>, EngineError> {
    let estimated = work.expected_duration_hours();
    let Some(estimated) = (estimated > 0.0).then_some(estimated) else {
        return Ok(Err(ExclusionReason::Unestimated));
    };
    let Some(started) = work.execution.events.started_at else {
        return Ok(Err(ExclusionReason::StartUnrecorded));
    };
    let Some(submitted) = verified_submission(work) else {
        return Ok(Err(ExclusionReason::SubmitUnrecorded));
    };
    let start_op = index.last(work.id, |c| matches!(c, Command::Start { .. }));
    let submit_op = index
        .of(work.id)
        .iter()
        .rev()
        .find(|op| matches!(op.command, Command::Submit { .. }) && occurred(op) == submitted)
        .copied();
    let (Some(_), Some(submit_op)) = (start_op, submit_op) else {
        return Ok(Err(ExclusionReason::NotInHistory));
    };
    if bulk.contains(&work.id) {
        return Ok(Err(ExclusionReason::BulkRecorded));
    }
    let actual = dpm_schedule::worked_between(plan, work, started, submitted)?;
    Ok(Ok(EstimateSample {
        key: work.key.clone(),
        executor: submit_op.actor.kind,
        capabilities: work.contract.capabilities.clone(),
        estimated_hours: estimated,
        actual_hours: actual,
        ratio: actual / estimated,
    }))
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

/// Whether the task's latest start and the first submission after it were committed within
/// [`BULK_WINDOW_SECONDS`] of each other, neither naming an occurrence time.
pub(super) fn bulk_recorded(index: &HistoryIndex<'_>, work: WorkItemId) -> bool {
    let operations = index.of(work);
    let Some(start) = operations
        .iter()
        .rposition(|op| matches!(op.command, Command::Start { .. }))
    else {
        return false;
    };
    let mut after = operations.iter().skip(start);
    let (Some(start), Some(submit)) = (
        after.next(),
        after.find(|op| matches!(op.command, Command::Submit { .. })),
    ) else {
        return false;
    };
    backfilled(&start.command).is_none()
        && backfilled(&submit.command).is_none()
        && (submit.timestamp - start.timestamp).num_seconds().abs() <= BULK_WINDOW_SECONDS
}

/// Ratio distributions per executor kind, or per executor kind and capability.
fn groups(samples: &[EstimateSample], by_capability: bool) -> Vec<RatioGroup> {
    let mut grouped: BTreeMap<(ActorKind, Option<&str>), Vec<f64>> = BTreeMap::new();
    for sample in samples {
        if by_capability {
            for capability in &sample.capabilities {
                grouped
                    .entry((sample.executor, Some(capability.as_str())))
                    .or_default()
                    .push(sample.ratio);
            }
        } else {
            grouped
                .entry((sample.executor, None))
                .or_default()
                .push(sample.ratio);
        }
    }
    grouped
        .into_iter()
        .map(|((executor, capability), ratios)| {
            let sorted = ascending(ratios);
            RatioGroup {
                executor,
                capability: capability.map(str::to_owned),
                samples: sorted.len(),
                median: percentile(&sorted, 0.5),
                p25: percentile(&sorted, 0.25),
                p75: percentile(&sorted, 0.75),
                sufficient: sorted.len() >= MIN_SAMPLES,
            }
        })
        .collect()
}
