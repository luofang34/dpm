use crate::ScheduleError;
use crate::network::{EPSILON, Network};
use crate::placement::Placement;
use crate::remaining_duration::RemainingDuration;
use crate::sampling::XorShift64;
use dpm_model::{Plan, WorkItemId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
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
    simulate_inner(&network, config, &durations, None)
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
    plan.validate()?;
    let remaining = crate::cpm::remaining_plan_at(plan, timeline);
    let durations = remaining.durations()?;
    let network = Network::compile(&remaining.plan)?;
    let expected: Vec<f64> = durations.iter().map(|(_, d)| d.expected()).collect();
    let mut placement = remaining.placement(&network, timeline, &expected)?;
    let mut summary = simulate_inner(&network, config, &durations, placement.as_mut())?;
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
    mut placement: Option<&mut Placement>,
) -> Result<SimulationSummary, ScheduleError> {
    if config.iterations == 0 {
        return Err(ScheduleError::NoSimulationIterations);
    }
    let mut rng = XorShift64::new(config.seed);
    let mut finishes = Vec::with_capacity(config.iterations);
    let mut critical_counts = vec![0_usize; network.order().len()];
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
        finishes.push(times.finish);
        for (at, count) in critical_counts.iter_mut().enumerate() {
            if network.total_float(&times, at)? <= EPSILON {
                *count = count.wrapping_add(1);
            }
        }
    }

    finishes.sort_by(f64::total_cmp);
    let criticality = network
        .order()
        .iter()
        .zip(critical_counts)
        .map(|(id, count)| (*id, count as f64 / config.iterations as f64))
        .collect();

    Ok(SimulationSummary {
        iterations: config.iterations,
        p50_finish_hours: percentile(&finishes, 0.50),
        p80_finish_hours: percentile(&finishes, 0.80),
        p95_finish_hours: percentile(&finishes, 0.95),
        criticality,
    })
}

#[cfg(test)]
mod tests;
