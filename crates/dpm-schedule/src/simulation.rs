use crate::network::Network;
use crate::placement::Placement;
use crate::remaining_duration::RemainingDuration;
use crate::sampling::XorShift64;
use crate::{RemainingOptions, ScheduleError};
use dpm_model::{Plan, WorkItemId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod samples;
use samples::{Samples, SortedSamples};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
/// Reproducible Monte Carlo sampling configuration.
pub struct SimulationConfig {
    /// Number of independently sampled schedules.
    pub iterations: usize,
    /// Deterministic pseudo-random seed.
    pub seed: u64,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            iterations: 10_000,
            seed: 0xDA6_51A9_2026,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Completion percentiles and per-activity criticality from sampled schedules.
pub struct SimulationSummary {
    /// Number of independently sampled schedules.
    pub iterations: usize,
    /// Median sampled completion time in elapsed hours.
    pub p50_finish_hours: f64,
    /// 80th percentile sampled completion time in elapsed hours.
    pub p80_finish_hours: f64,
    /// 95th percentile sampled completion time in elapsed hours.
    pub p95_finish_hours: f64,
    /// Fraction of sampled schedules in which each activity is critical.
    pub criticality: BTreeMap<WorkItemId, f64>,
    /// Sample standard deviation of the project finish in elapsed hours: the sampled spread,
    /// which includes every near-critical path.
    #[serde(default)]
    pub sampled_std_dev_hours: f64,
    /// Sampled finish percentiles of every milestone in the projection.
    #[serde(default)]
    pub milestones: BTreeMap<WorkItemId, MilestoneFinish>,
    /// Every sampled project finish, kept for horizon probabilities rather than serialized.
    #[serde(skip)]
    finishes: SortedSamples,
}

impl SimulationSummary {
    /// Fraction of sampled schedules whose project finish is at or before `horizon_hours`.
    #[must_use]
    pub fn probability_by(&self, horizon_hours: f64) -> f64 {
        self.finishes.probability_by(horizon_hours)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Sampled finish percentiles of one milestone, in elapsed hours.
pub struct MilestoneFinish {
    /// Median sampled finish.
    pub p50_finish_hours: f64,
    /// 80th percentile sampled finish.
    pub p80_finish_hours: f64,
    /// 95th percentile sampled finish.
    pub p95_finish_hours: f64,
    /// Every sampled finish, kept for horizon probabilities rather than serialized.
    #[serde(skip)]
    samples: SortedSamples,
}

impl MilestoneFinish {
    fn new(samples: SortedSamples) -> Self {
        Self {
            p50_finish_hours: samples.percentile(0.50),
            p80_finish_hours: samples.percentile(0.80),
            p95_finish_hours: samples.percentile(0.95),
            samples,
        }
    }

    /// Fraction of sampled schedules in which the milestone is reached by `horizon_hours`.
    #[must_use]
    pub fn probability_by(&self, horizon_hours: f64) -> f64 {
        self.samples.probability_by(horizon_hours)
    }
}

/// Milestones of a compiled plan with their topological positions.
fn milestone_positions(
    plan: &Plan,
    network: &Network,
) -> Result<Vec<(WorkItemId, usize)>, ScheduleError> {
    plan.work_items
        .values()
        .filter(|work| work.kind == dpm_model::WorkKind::Milestone)
        .map(|work| {
            let at = network
                .position(&work.id)
                .ok_or(ScheduleError::MissingWorkItem(work.id))?;
            Ok((work.id, at))
        })
        .collect()
}

/// Linearly interpolated percentile `p` (0..=1) of ascending values; 0 for no values.
///
/// Every consumer that summarizes samples uses this one definition, so results are reproducible.
#[must_use]
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = p.clamp(0.0, 1.0) * (sorted.len().saturating_sub(1) as f64);
    let low = rank.floor() as usize;
    let high = rank.ceil() as usize;
    // Both ranks lie within the non-empty slice by construction.
    match (sorted.get(low), sorted.get(high)) {
        (Some(below), _) if low == high => *below,
        (Some(below), Some(above)) => {
            let fraction = rank - low as f64;
            below * (1.0 - fraction) + above * fraction
        }
        _ => 0.0,
    }
}

/// Sample beta-PERT task durations and project complete baseline schedules.
///
/// Each task's samples have the PERT expectation that `deterministic` projects as their mean.
pub fn simulate(plan: &Plan, config: SimulationConfig) -> Result<SimulationSummary, ScheduleError> {
    plan.validate()?;
    let durations = plan
        .work_items
        .iter()
        .map(|(id, work)| Ok((*id, RemainingDuration::of(*id, work, false, 0.0)?)))
        .collect::<Result<Vec<_>, ScheduleError>>()?;
    let network = Network::compile(plan)?;
    let milestones = milestone_positions(plan, &network)?;
    simulate_inner(&network, config, &durations, milestones, None)
}

/// Monte Carlo projection of remaining applicable work from the execution state at a clock reading.
///
/// It samples the same active graph and the same remaining durations as `deterministic_remaining`,
/// whose values are the means of these samples; excluded work has no criticality.
pub fn simulate_remaining(
    plan: &Plan,
    config: SimulationConfig,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<SimulationSummary, ScheduleError> {
    simulate_remaining_at(plan, config, &dpm_model::Timeline::at(plan, now))
}

/// Sample the remaining graph sharing the gate timeline for the same immutable plan snapshot.
pub fn simulate_remaining_at(
    plan: &Plan,
    config: SimulationConfig,
    timeline: &dpm_model::Timeline,
) -> Result<SimulationSummary, ScheduleError> {
    simulate_remaining_with(plan, config, timeline, RemainingOptions::default())
}

/// Sample the remaining graph with caller-requested adjustments, the same ones
/// [`deterministic_remaining_with`](crate::deterministic_remaining_with) applies.
pub fn simulate_remaining_with(
    plan: &Plan,
    config: SimulationConfig,
    timeline: &dpm_model::Timeline,
    options: RemainingOptions,
) -> Result<SimulationSummary, ScheduleError> {
    options.validate()?;
    plan.validate()?;
    let remaining = crate::cpm::remaining_plan_at(plan, timeline)?;
    let durations = remaining.durations()?;
    let network = Network::compile(&remaining.plan)?;
    let expected: Vec<f64> = durations.iter().map(|(_, d)| d.expected()).collect();
    let mut placement = remaining.placement(&network, timeline, &expected, options)?;
    let milestones = milestone_positions(&remaining.plan, &network)?
        .into_iter()
        .filter(|(id, _)| !remaining.excluded.contains(id))
        .collect();
    let mut summary = simulate_inner(&network, config, &durations, milestones, placement.as_mut())?;
    summary
        .criticality
        .retain(|id, _| !remaining.excluded.contains(id));
    Ok(summary)
}

/// Sample every activity's duration source, in work-item id order, and project each sample.
fn simulate_inner(
    network: &Network,
    config: SimulationConfig,
    durations_by_id: &[(WorkItemId, RemainingDuration)],
    milestones: Vec<(WorkItemId, usize)>,
    mut placement: Option<&mut Placement>,
) -> Result<SimulationSummary, ScheduleError> {
    if config.iterations == 0 {
        return Err(ScheduleError::NoSimulationIterations);
    }
    let mut rng = XorShift64::new(config.seed);
    let mut samples = Samples::new(network, config.iterations, milestones);
    let mut durations = vec![0.0; network.order().len()];
    // Sampling follows work-item id order, so a seed draws the same numbers for the same plan.
    let mut sampled = Vec::with_capacity(durations_by_id.len());
    for (id, source) in durations_by_id {
        let at = network
            .position(id)
            .ok_or(ScheduleError::MissingWorkItem(*id))?;
        sampled.push((at, *id, source));
    }
    for _ in 0..config.iterations {
        for (at, id, source) in &sampled {
            let slot = durations
                .get_mut(*at)
                .ok_or(ScheduleError::UnknownPosition(*at))?;
            *slot = source.sample(&mut rng);
            if !slot.is_finite() || *slot < 0.0 {
                return Err(ScheduleError::InvalidDuration(*id));
            }
        }
        let times = match placement.as_deref_mut() {
            Some(placement) => network.times_placed(&durations, placement)?,
            None => network.times(&durations)?,
        };
        samples.record(network, &durations, &times)?;
    }
    Ok(samples.summarize(network, config.iterations))
}

#[cfg(test)]
mod tests;
