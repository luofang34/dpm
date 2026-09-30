//! Calibration of estimates, review and decision waits, and flow, measured from recorded history.
//!
//! Everything here is derived from the snapshot's recorded event times and the operation log the
//! adapter passes in; nothing is stored and no estimate is ever rewritten. A forecast applies the
//! measured factors only when a caller asks for it ([`crate::status_calibrated`]).

use crate::{EngineError, Operation};
use chrono::{DateTime, Utc};
use dpm_model::{ActorKind, Key, Plan};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod forecast;
mod history;
mod samples;
mod waits;
pub(crate) use forecast::calibrated_forecast;
pub use forecast::{AppliedCalibration, AppliedFactor, AppliedReviewDelay};
pub(crate) use history::HistoryIndex;

/// Samples a group needs before a calibrated forecast applies its median.
pub const MIN_SAMPLES: usize = 5;

/// Start and submit operations committed this close together, neither naming an occurrence
/// time, were recorded after the fact in one sitting, so their interval says nothing about the
/// work.
pub const BULK_WINDOW_SECONDS: i64 = 180;

/// Actual working time below this is no measurement: start and submit were recorded together, or
/// the whole interval fell outside the task's working hours.
pub const MIN_ACTUAL_SECONDS: i64 = 60;

/// Smallest group median a calibrated forecast applies; anything lower says the records, not the
/// estimates, are wrong.
pub const MIN_APPLIED_RATIO: f64 = 0.02;

/// Largest group median a calibrated forecast applies.
pub const MAX_APPLIED_RATIO: f64 = 50.0;

/// A review recorded this soon after its submission was recorded together with it; no one reads
/// and judges a result that fast, so it is no wait.
pub const REVIEW_FLOOR_SECONDS: i64 = 10;

/// Calibration of estimates and waits, and flow metrics, at one clock reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationReport {
    /// Clock reading the report was computed at.
    pub generated_at: DateTime<Utc>,
    /// Thresholds and units every figure in the report follows.
    pub rules: CalibrationRules,
    /// How much operation history the report could read.
    pub history: HistoryCoverage,
    /// Actual working time against estimates, per executor kind and capability.
    pub estimates: EstimateCalibration,
    /// Time from each submission to its review, per reviewer kind.
    pub reviews: WaitReport,
    /// Time from a decision's opening to its resolution, per deciding actor kind.
    pub decisions: WaitReport,
    /// Cycle time, lead time, throughput, aging work in progress and claim reliability.
    pub flow: crate::FlowReport,
}

/// Thresholds and units of a calibration report.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CalibrationRules {
    /// Samples a group needs before a calibrated forecast applies it.
    pub min_samples: usize,
    /// Claim, start and submit of one attempt committed within this many seconds, start and
    /// submit without an occurrence time, count as recorded in bulk.
    pub bulk_window_seconds: i64,
    /// Samples with less actual time than this are excluded as `no_working_time`.
    pub min_actual_seconds: i64,
    /// Smallest group median a calibrated forecast applies.
    pub min_applied_ratio: f64,
    /// Largest group median a calibrated forecast applies.
    pub max_applied_ratio: f64,
    /// Reviews recorded within this many seconds of their submission are not waits.
    pub review_floor_seconds: i64,
    /// Unit of actual execution time.
    pub actual_hours: HourBasis,
}

/// How actual execution time is counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HourBasis {
    /// Working hours of each task's calendar, the unit its estimate is written in.
    Working,
    /// Elapsed hours, because the plan has no calendars.
    Elapsed,
}

/// Extent of the operation log the report read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryCoverage {
    /// Operations read.
    pub operations: usize,
    /// Commit time of the first operation, if any.
    pub first_at: Option<DateTime<Utc>>,
}

/// Actual against estimated execution time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimateCalibration {
    /// Every verified task measured, in key order.
    pub samples: Vec<EstimateSample>,
    /// Verified tasks that were not measured, grouped by reason.
    pub excluded: Vec<Exclusion>,
    /// Ratio distribution per executor kind.
    pub by_executor: Vec<RatioGroup>,
    /// Ratio distribution per executor kind and required capability; a task counts once for each
    /// capability it requires.
    pub by_capability: Vec<RatioGroup>,
}

/// One verified task's actual execution time against its estimate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimateSample {
    /// Task key.
    pub key: Key,
    /// Kind of the actor who submitted the verified attempt.
    pub executor: ActorKind,
    /// Capabilities the task requires.
    pub capabilities: BTreeSet<String>,
    /// PERT expected hours of the estimate.
    pub estimated_hours: f64,
    /// Hours from the start to the verified submission, in the report's hour basis, less the
    /// review waits of the task's earlier rejected attempts.
    pub actual_hours: f64,
    /// `actual_hours / estimated_hours`.
    pub ratio: f64,
}

/// Distribution of actual/estimated ratios in one group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RatioGroup {
    /// Executor kind of the samples.
    pub executor: ActorKind,
    /// Capability of the samples, for a per-capability group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// Number of samples.
    pub samples: usize,
    /// Median ratio, the factor a calibrated forecast applies; absent without samples.
    pub median: Option<f64>,
    /// 25th percentile ratio; absent without samples.
    pub p25: Option<f64>,
    /// 75th percentile ratio; absent without samples.
    pub p75: Option<f64>,
    /// Whether the group has at least [`MIN_SAMPLES`] samples.
    pub sufficient: bool,
    /// Verified tasks of this executor kind (and capability) left out, per reason; what they
    /// would have said is missing from the distribution.
    pub excluded: Vec<ReasonCount>,
}

/// How many items one reason left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasonCount {
    /// Why they were left out.
    pub reason: ExclusionReason,
    /// How many were left out.
    pub count: usize,
}

/// Why recorded work or a decision was left out of a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    /// The task has no estimate, or its expected hours are zero.
    Unestimated,
    /// The task's start time was never recorded.
    StartUnrecorded,
    /// The verified submission's time was never recorded.
    SubmitUnrecorded,
    /// The start or the verified submission is not in the operation log the store holds.
    NotInHistory,
    /// Claim, start and submit were committed within the bulk window, start and submit without an
    /// occurrence time; with the claim before the log begins, start and submit alone.
    BulkRecorded,
    /// Actors of more than one kind held the attempt between its start and the verified
    /// submission, so no one kind's ratio describes it.
    MixedExecutors,
    /// The task's calendar cannot count the working hours of its recorded interval.
    CalendarOutOfRange,
    /// Less than [`MIN_ACTUAL_SECONDS`] of actual time: the interval is empty or lies outside
    /// the task's working hours.
    NoWorkingTime,
    /// Work verified before submission attempts were recorded has no reviewer or review time.
    AttemptUnrecorded,
    /// The review was recorded within [`REVIEW_FLOOR_SECONDS`] of its submission.
    ReviewedOnSubmission,
    /// The decision existed before the operation log begins, so its opening time is unknown.
    OpenedBeforeHistory,
    /// A reviewed replacement is created already decided; it never waited.
    Replacement,
    /// The decision's resolution time was never recorded.
    ResolutionUnrecorded,
    /// The task's verification time was never recorded.
    VerificationUnrecorded,
    /// The task's first claim is not in the operation log the store holds.
    ClaimedBeforeHistory,
}

/// Items left out of one measurement for one reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exclusion {
    /// Why they were left out.
    pub reason: ExclusionReason,
    /// How many were left out; for reviews, the number of reviews.
    pub count: usize,
    /// Their keys, in key order, each once.
    pub keys: Vec<Key>,
}

/// Waits grouped by the kind of actor that ended them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaitReport {
    /// Wait distribution per actor kind.
    pub by_kind: Vec<WaitGroup>,
    /// Items not measured, grouped by reason.
    pub excluded: Vec<Exclusion>,
}

/// Distribution of elapsed waits for one actor kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaitGroup {
    /// Kind of the reviewing or deciding actor.
    pub kind: ActorKind,
    /// Number of waits.
    pub count: usize,
    /// Median elapsed hours, the delay a calibrated forecast applies for reviews.
    pub median_hours: f64,
    /// 80th percentile elapsed hours.
    pub p80_hours: f64,
    /// Whether the group has at least [`MIN_SAMPLES`] waits.
    pub sufficient: bool,
}

/// Measure estimates, waits and flow from a validated plan and its operation log in append order.
///
/// `history` is the log the plan's snapshot results from; items whose facts predate it are
/// reported as excluded rather than guessed.
pub fn calibration(
    plan: &Plan,
    history: &[Operation],
    now: DateTime<Utc>,
) -> Result<CalibrationReport, EngineError> {
    plan.validate()?;
    let index = HistoryIndex::new(history);
    let measured = samples::measure(plan, &index);
    Ok(CalibrationReport {
        generated_at: now,
        rules: CalibrationRules {
            min_samples: MIN_SAMPLES,
            bulk_window_seconds: BULK_WINDOW_SECONDS,
            min_actual_seconds: MIN_ACTUAL_SECONDS,
            min_applied_ratio: MIN_APPLIED_RATIO,
            max_applied_ratio: MAX_APPLIED_RATIO,
            review_floor_seconds: REVIEW_FLOOR_SECONDS,
            actual_hours: if plan.calendars.is_some() {
                HourBasis::Working
            } else {
                HourBasis::Elapsed
            },
        },
        history: HistoryCoverage {
            operations: history.len(),
            first_at: history.first().map(|op| op.timestamp),
        },
        reviews: waits::reviews(plan, &measured.bulk),
        decisions: waits::decisions(plan, &index),
        flow: crate::query::flow::flow(plan, &index, &measured.bulk, now),
        estimates: measured.calibration,
    })
}

/// Group excluded keys by reason, reasons and keys in order.
pub(crate) fn exclusions(mut items: Vec<(ExclusionReason, Key)>) -> Vec<Exclusion> {
    items.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.natural_cmp(&b.1)));
    let mut grouped: Vec<Exclusion> = Vec::new();
    for (reason, key) in items {
        match grouped.last_mut() {
            Some(last) if last.reason == reason => {
                last.count = last.count.wrapping_add(1);
                if last.keys.last() != Some(&key) {
                    last.keys.push(key);
                }
            }
            _ => grouped.push(Exclusion {
                reason,
                count: 1,
                keys: vec![key],
            }),
        }
    }
    grouped
}

/// Values in ascending order for [`dpm_schedule::percentile`].
pub(crate) fn ascending(mut values: Vec<f64>) -> Vec<f64> {
    values.sort_by(f64::total_cmp);
    values
}

/// Elapsed hours from `from` to `to`.
pub(crate) fn elapsed_hours(from: DateTime<Utc>, to: DateTime<Utc>) -> f64 {
    (to - from).num_milliseconds() as f64 / 3_600_000.0
}

#[cfg(test)]
pub(crate) mod tests;
