//! A forecast that applies measured factors to a copy of the plan, on request only.

use super::{CalibrationReport, MIN_SAMPLES};
use dpm_model::{ActorKind, MAX_CALENDAR_HOURS, Plan, ThreePointEstimate, WorkItem};
use dpm_schedule::RemainingOptions;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The factors a calibrated `status` applied to a copy of the plan; the stored plan and its
/// estimates are unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedCalibration {
    /// Samples a group needed before its measurement was applied.
    pub min_samples: usize,
    /// One entry per executor kind that unfinished estimated work resolves to.
    pub factors: Vec<AppliedFactor>,
    /// Review wait added after the work of every task still awaiting verification.
    pub review_delay: AppliedReviewDelay,
    /// Tasks whose scaled estimate was capped at the largest estimate a plan with calendars may
    /// state.
    pub capped: usize,
}

/// The estimate factor for one executor kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedFactor {
    /// Kind the work resolves to: its owner's, else its planned executor, else the calendars'
    /// default executor, else human.
    pub executor: ActorKind,
    /// Unfinished estimated tasks of that kind.
    pub tasks: usize,
    /// Samples of that kind in the calibration.
    pub samples: usize,
    /// Median actual/estimated ratio measured, if any sample exists.
    pub measured: Option<f64>,
    /// Factor multiplied into optimistic, most-likely and pessimistic hours; 1 when not applied.
    pub factor: f64,
    /// Whether the measured median was applied.
    pub applied: bool,
    /// Why the factor is what it is.
    pub reason: String,
}

/// The review wait added for tasks still awaiting verification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedReviewDelay {
    /// Kind that reviews: the calendars' verifier, else human.
    pub verifier: ActorKind,
    /// Reviews by that kind in the calibration.
    pub samples: usize,
    /// Median elapsed review wait measured, if any review exists.
    pub measured_hours: Option<f64>,
    /// Elapsed hours added after each such task's work; 0 when not applied.
    pub hours: f64,
    /// Whether the measured median was applied.
    pub applied: bool,
    /// Why the delay is what it is.
    pub reason: String,
}

/// Executor kind the forecast attributes to work.
fn executor(plan: &Plan, work: &WorkItem) -> ActorKind {
    work.execution
        .owner
        .as_ref()
        .map(|o| o.kind)
        .or(work.schedule.executor)
        .or(plan.calendars.as_ref().map(|c| c.default_executor))
        .unwrap_or(ActorKind::Human)
}

/// A copy of the plan with each unfinished task's estimate scaled by its executor kind's measured
/// median, the review delay to project with, and what was applied.
pub(crate) fn calibrated_forecast(
    plan: &Plan,
    report: &CalibrationReport,
) -> (Plan, RemainingOptions, AppliedCalibration) {
    let mut scaled = plan.clone();
    let mut tasks: BTreeMap<ActorKind, usize> = BTreeMap::new();
    for work in plan.work_items.values().filter(|w| unfinished_estimate(w)) {
        let count = tasks.entry(executor(plan, work)).or_default();
        *count = count.wrapping_add(1);
    }
    let factors: Vec<AppliedFactor> = tasks
        .into_iter()
        .map(|(kind, tasks)| factor(report, kind, tasks))
        .collect();
    let limit = plan.calendars.as_ref().map(|_| MAX_CALENDAR_HOURS);
    let mut capped = 0_usize;
    for work in scaled
        .work_items
        .values_mut()
        .filter(|w| unfinished_estimate(w))
    {
        let kind = executor(plan, work);
        let applied = factors
            .iter()
            .find(|f| f.executor == kind)
            .map(|f| f.factor);
        if let (Some(estimate), Some(factor)) = (work.schedule.estimate.as_mut(), applied)
            && scale(estimate, factor, limit)
        {
            capped = capped.wrapping_add(1);
        }
    }
    let review_delay = review_delay(plan, report);
    let options = RemainingOptions {
        review_delay_hours: review_delay.hours,
    };
    let applied = AppliedCalibration {
        min_samples: MIN_SAMPLES,
        factors,
        review_delay,
        capped,
    };
    (scaled, options, applied)
}

fn unfinished_estimate(work: &WorkItem) -> bool {
    work.is_executable()
        && work.schedule.estimate.is_some()
        && !work.execution.status.satisfies_dependency()
}

/// Scale an estimate; `true` when a bound capped it.
fn scale(estimate: &mut ThreePointEstimate, factor: f64, limit: Option<f64>) -> bool {
    let mut capped = false;
    for hours in [
        &mut estimate.optimistic_hours,
        &mut estimate.likely_hours,
        &mut estimate.pessimistic_hours,
    ] {
        *hours *= factor;
        if let Some(limit) = limit.filter(|limit| *hours > *limit) {
            *hours = limit;
            capped = true;
        }
    }
    capped
}

fn factor(report: &CalibrationReport, executor: ActorKind, tasks: usize) -> AppliedFactor {
    let group = report
        .estimates
        .by_executor
        .iter()
        .find(|g| g.executor == executor);
    let samples = group.map_or(0, |g| g.samples);
    let measured = group.map(|g| g.median);
    let usable = measured.filter(|m| m.is_finite() && *m >= 0.0);
    let (factor, applied, reason) = match (group, usable) {
        (Some(g), Some(median)) if g.sufficient => (
            median,
            true,
            format!(
                "median actual/estimated ratio of {samples} verified {executor:?} task(s) applied"
            ),
        ),
        _ => (
            1.0,
            false,
            format!(
                "{samples} verified {executor:?} sample(s), fewer than the {MIN_SAMPLES} needed; estimates kept"
            ),
        ),
    };
    AppliedFactor {
        executor,
        tasks,
        samples,
        measured,
        factor,
        applied,
        reason,
    }
}

fn review_delay(plan: &Plan, report: &CalibrationReport) -> AppliedReviewDelay {
    let verifier = plan
        .calendars
        .as_ref()
        .map_or(ActorKind::Human, |c| c.verifier);
    let group = report.reviews.by_kind.iter().find(|g| g.kind == verifier);
    let samples = group.map_or(0, |g| g.count);
    let measured_hours = group.map(|g| g.median_hours);
    let usable = measured_hours.filter(|h| h.is_finite() && *h >= 0.0);
    let (hours, applied, reason) = match (group, usable) {
        (Some(g), Some(median)) if g.sufficient => (
            median,
            true,
            format!(
                "median wait of {samples} {verifier:?} review(s) added after each unverified task's work"
            ),
        ),
        _ => (
            0.0,
            false,
            format!(
                "{samples} {verifier:?} review(s) measured, fewer than the {MIN_SAMPLES} needed; no review delay added"
            ),
        ),
    };
    AppliedReviewDelay {
        verifier,
        samples,
        measured_hours,
        hours,
        applied,
        reason,
    }
}
