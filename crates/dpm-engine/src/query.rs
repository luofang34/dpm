use crate::{EngineError, Transition, gates};
use chrono::{DateTime, Utc};
use dpm_model::{
    Applicability, BasisStatus, DecisionStatus, Plan, Timeline, WorkItem, WorkItemId, WorkStatus,
    basis_status,
};
use dpm_schedule::{
    RemainingOptions, SimulationConfig, SimulationSummary, deterministic_remaining_with,
    simulate_remaining_with,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(crate) mod choices;
pub use choices::{InapplicableWork, MAX_SCENARIOS, OpenChoices, ScenarioForecast};
mod estimates;
pub use estimates::{is_unestimated, unestimated};
mod explain;
pub use explain::{BasisReport, WorkExplanation, explain_work};
mod advisory;
pub use advisory::{Advisory, advisories};
mod calibration;
pub use calibration::{
    AppliedCalibration, AppliedFactor, AppliedReviewDelay, BULK_WINDOW_SECONDS, CalibrationReport,
    CalibrationRules, EstimateCalibration, EstimateSample, Exclusion, ExclusionReason,
    HistoryCoverage, HourBasis, MAX_APPLIED_RATIO, MIN_ACTUAL_SECONDS, MIN_AGENT_APPLIED_RATIO,
    MIN_APPLIED_RATIO, MIN_SAMPLES, REVIEW_FLOOR_SECONDS, RatioGroup, ReasonCount, WaitGroup,
    WaitReport, calibration, min_applied_ratio,
};
mod forecast;
pub use forecast::{
    ForecastSpread, MilestoneForecast, PathStep, PertApproximationReport, RiskForecast,
};
mod next;
pub use next::{CriticalityBasis, NextWorkCandidate, NextWorkQuery, next_work};
mod flow;
pub use flow::{
    AgingWork, ClaimOutcomes, Durations, FlowReport, HolderOutcomes, Reliability, THROUGHPUT_WEEKS,
    Throughput, WeekCount,
};
#[derive(Debug, Clone, Serialize, Deserialize)]
/// Execution counts and remaining schedule projections.
pub struct StatusSummary {
    /// Claim eligibility of each leaf task, using the same policy as commands and next.
    pub gates: BTreeMap<WorkItemId, crate::GateReport>,
    /// Reported execution progress, separate from verified completion.
    pub progress: crate::ProgressSummary,
    /// Authoritative wrapping revision used for this projection.
    pub revision: u64,
    /// Count of all tasks, milestones, and containers.
    pub total_work: usize,
    /// Number of tasks that can be claimed now.
    pub ready: usize,
    /// Number of explicitly blocked tasks within the counted scope.
    pub blocked: usize,
    /// Number of claimed or in-progress tasks within the counted scope.
    pub in_flight: usize,
    /// Number of tasks awaiting independent verification within the counted scope.
    pub awaiting_verification: usize,
    /// Claimed, in-progress, blocked or submitted tasks outside the counted scope: a reviewed
    /// choice change excluded them, so they take no transition until the plan changes again.
    ///
    /// The counted scope is the one progress and `complete` use, so no lifecycle count includes
    /// work that no longer counts toward completion.
    #[serde(default)]
    pub excluded_in_flight: usize,
    /// Number of tasks whose enforced provisional basis rests on a rejected predecessor attempt.
    #[serde(default)]
    pub basis_invalidated: usize,
    /// Count of completed tasks and derived aggregate completions.
    pub complete: usize,
    /// Number of unresolved decisions, whether contextual or blocking.
    pub open_decisions: usize,
    /// Deterministic remaining duration of the active graph in elapsed hours; while choices are
    /// open it covers committed work only, and `open_choices` forecasts each scenario.
    pub expected_finish_hours: f64,
    /// Median sampled completion time in elapsed hours; withheld while choices are open.
    pub p50_finish_hours: Option<f64>,
    /// 80th percentile sampled completion time in elapsed hours; withheld while choices are open.
    pub p80_finish_hours: Option<f64>,
    /// 95th percentile sampled completion time in elapsed hours; withheld while choices are open.
    pub p95_finish_hours: Option<f64>,
    /// Milestone percentiles, horizon probabilities and the analytic PERT approximation; null
    /// whenever the percentiles are withheld.
    #[serde(default)]
    pub forecast: Option<RiskForecast>,
    /// Tasks the headline forecast counts as 0 h because they have no estimate, in key order
    /// ([`is_unestimated`](crate::is_unestimated)); the forecast is optimistic by their durations.
    #[serde(default)]
    pub unestimated: Vec<dpm_model::Key>,
    /// Work outside the active graph, with the reason; it is in no forecast and never ready.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_applicable: Vec<InapplicableWork>,
    /// Open decisions that condition work, with one forecast per option combination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_choices: Option<OpenChoices>,
    /// Measured factors the forecasts applied, present only for a calibrated status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration: Option<AppliedCalibration>,
}

pub(crate) fn simulation_config() -> SimulationConfig {
    SimulationConfig {
        iterations: 2_000,
        ..SimulationConfig::default()
    }
}

/// Claim readiness of every leaf task, evaluated once for one timeline.
fn claim_reports(plan: &Plan, timeline: &Timeline) -> BTreeMap<WorkItemId, crate::GateReport> {
    plan.work_items
        .values()
        .filter(|w| w.is_executable())
        .map(|w| (w.id, gates::evaluate(plan, w, Transition::Claim, timeline)))
        .collect()
}

/// Project validated lifecycle counts and remaining duration at an adapter-supplied time.
pub fn status(
    plan: &Plan,
    probabilistic: bool,
    now: DateTime<Utc>,
) -> Result<StatusSummary, EngineError> {
    status_with(plan, probabilistic, now, None, None)
}

/// [`status`] whose forecast also states the probability of finishing the project, and of
/// reaching each milestone, within `horizon_hours` elapsed hours of `now`.
pub fn status_at_horizon(
    plan: &Plan,
    probabilistic: bool,
    now: DateTime<Utc>,
    horizon_hours: Option<f64>,
) -> Result<StatusSummary, EngineError> {
    status_with(plan, probabilistic, now, None, horizon_hours)
}

/// [`status`] whose forecasts scale each unfinished task's estimate by its executor kind's
/// measured ratio and add the verifier kind's measured review wait, each only when enough samples
/// support it; `calibration` in the result states what was applied and why. The plan itself is
/// never changed.
pub fn status_calibrated(
    plan: &Plan,
    probabilistic: bool,
    now: DateTime<Utc>,
    report: &CalibrationReport,
) -> Result<StatusSummary, EngineError> {
    status_with(plan, probabilistic, now, Some(report), None)
}

/// [`status_calibrated`] with the horizon probabilities of [`status_at_horizon`].
pub fn status_calibrated_at_horizon(
    plan: &Plan,
    probabilistic: bool,
    now: DateTime<Utc>,
    report: &CalibrationReport,
    horizon_hours: Option<f64>,
) -> Result<StatusSummary, EngineError> {
    status_with(plan, probabilistic, now, Some(report), horizon_hours)
}

/// The seeded simulation of `forecast`, unless it is not requested, no work of `plan` is
/// estimated, or open choices leave mutually exclusive branches without a probability model.
fn sampled(
    (plan, forecast): (&Plan, &Plan),
    timeline: &Timeline,
    options: RemainingOptions,
    probabilistic: bool,
    open_choices: Option<&OpenChoices>,
) -> Result<Option<SimulationSummary>, EngineError> {
    let estimated = plan
        .work_items
        .values()
        .any(|w| w.schedule.estimate.is_some());
    if !probabilistic || open_choices.is_some() || !estimated {
        return Ok(None);
    }
    Ok(Some(simulate_remaining_with(
        forecast,
        simulation_config(),
        timeline,
        options,
    )?))
}

fn status_with(
    plan: &Plan,
    probabilistic: bool,
    now: DateTime<Utc>,
    report: Option<&CalibrationReport>,
    horizon_hours: Option<f64>,
) -> Result<StatusSummary, EngineError> {
    plan.validate()?;
    forecast::validate_horizon(horizon_hours)?;
    let timeline = Timeline::at(plan, now);
    let calibrated = report.map(|r| calibration::calibrated_forecast(plan, r));
    let (forecast, options) = match &calibrated {
        Some((scaled, options, _)) => (scaled, *options),
        None => (plan, RemainingOptions::default()),
    };
    let forecast_timeline = Timeline::at(forecast, now);
    let schedule = deterministic_remaining_with(forecast, &forecast_timeline, options)?;
    let open_choices = choices::open_choices(forecast, probabilistic, now, options)?;
    let simulation = sampled(
        (plan, forecast),
        &forecast_timeline,
        options,
        probabilistic,
        open_choices.as_ref(),
    )?;
    let risk = simulation
        .as_ref()
        .map(|sampled| {
            let inputs = (forecast, &forecast_timeline, options);
            forecast::risk_forecast(plan, inputs, sampled, horizon_hours)
        })
        .transpose()?;
    let gates = claim_reports(plan, &timeline);

    Ok(StatusSummary {
        ready: gates.values().filter(|report| report.ready).count(),
        gates,
        progress: crate::progress::progress_with(plan, &timeline).overall,
        revision: plan.revision,
        total_work: plan.work_items.len(),
        blocked: count(plan, &timeline, true, &[WorkStatus::Blocked]),
        in_flight: count(
            plan,
            &timeline,
            true,
            &[WorkStatus::Claimed, WorkStatus::InProgress],
        ),
        awaiting_verification: count(plan, &timeline, true, &[WorkStatus::Submitted]),
        excluded_in_flight: excluded_in_flight(plan, &timeline).len(),
        basis_invalidated: plan
            .work_items
            .values()
            .filter(|w| basis_status(plan, w).iter().any(BasisStatus::gates))
            .count(),
        complete: timeline.completed().len(),
        open_decisions: plan
            .decisions
            .values()
            .filter(|decision| decision.status == DecisionStatus::Open)
            .count(),
        expected_finish_hours: schedule.project_finish_hours,
        p50_finish_hours: simulation.as_ref().map(|s| s.p50_finish_hours),
        p80_finish_hours: simulation.as_ref().map(|s| s.p80_finish_hours),
        p95_finish_hours: simulation.as_ref().map(|s| s.p95_finish_hours),
        forecast: risk,
        unestimated: unestimated(plan, &timeline),
        not_applicable: choices::not_applicable(plan, &timeline),
        open_choices,
        calibration: calibrated.map(|(_, _, applied)| applied),
    })
}

/// Tasks in one of `statuses`, inside (`counted`) or outside the status scope.
fn count(plan: &Plan, timeline: &Timeline, counted: bool, statuses: &[WorkStatus]) -> usize {
    plan.work_items
        .values()
        .filter(|w| statuses.contains(&w.execution.status))
        .filter(|w| in_status_scope(timeline, w.id) == counted)
        .count()
}

/// Whether work's lifecycle counts in `status` lifecycle counts, `complete` and progress: its own
/// conditions are selected. Views that list work behind those counts use this scope, so a list
/// never shows work its count leaves out.
#[must_use]
pub fn in_status_scope(timeline: &Timeline, work: WorkItemId) -> bool {
    timeline.applicability(work).condition_selected()
}

/// Claimed, started, blocked or submitted work outside the status scope: a reviewed choice change
/// excluded it, so it takes no transition until the plan changes again. `status` counts it as
/// `excluded_in_flight`.
#[must_use]
pub fn excluded_in_flight<'a>(plan: &'a Plan, timeline: &Timeline) -> Vec<&'a WorkItem> {
    const IN_FLIGHT: [WorkStatus; 4] = [
        WorkStatus::Claimed,
        WorkStatus::InProgress,
        WorkStatus::Blocked,
        WorkStatus::Submitted,
    ];
    plan.work_items
        .values()
        .filter(|w| IN_FLIGHT.contains(&w.execution.status) && !in_status_scope(timeline, w.id))
        .collect()
}

/// Transitive successors that completing work could still release; excluded or stranded work
/// is not counted because no completion releases it. Only enforced edges count: a waived edge
/// gates nothing, so ranking must not reward releasing it.
fn downstream_counts(
    plan: &Plan,
    timeline: &Timeline,
    starts: impl IntoIterator<Item = WorkItemId>,
) -> BTreeMap<WorkItemId, usize> {
    let mut outgoing = BTreeMap::<WorkItemId, Vec<WorkItemId>>::new();
    for dep in plan.enforced_dependencies().filter(|d| {
        let state = timeline.applicability(d.successor);
        !(state.is_not_selected()
            || matches!(
                state,
                Applicability::Stranded { .. }
                    | Applicability::ChildrenStranded { .. }
                    | Applicability::EmptyJoin
            ))
    }) {
        outgoing
            .entry(dep.predecessor)
            .or_default()
            .push(dep.successor);
    }

    let mut result = BTreeMap::new();
    for start in starts {
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([start]);
        while let Some(node) = queue.pop_front() {
            if let Some(next) = outgoing.get(&node) {
                for child in next {
                    if seen.insert(*child) {
                        queue.push_back(*child);
                    }
                }
            }
        }
        result.insert(start, seen.len());
    }
    result
}
