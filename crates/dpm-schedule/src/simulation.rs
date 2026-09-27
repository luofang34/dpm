use crate::ScheduleError;
use crate::network::{EPSILON, Network};
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

#[derive(Debug, Clone, Copy)]
struct XorShift64(u64);

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn unit_f64(&mut self) -> f64 {
        // 53 random bits, in [0, 1).
        ((self.next_u64() >> 11) as f64) * (1.0 / ((1_u64 << 53) as f64))
    }
}

fn sample_triangular(rng: &mut XorShift64, a: f64, m: f64, b: f64) -> f64 {
    if (b - a).abs() <= EPSILON {
        return a;
    }
    let u = rng.unit_f64();
    let mode_fraction = (m - a) / (b - a);
    if u < mode_fraction {
        a + (b - a) * (u * mode_fraction).sqrt()
    } else {
        b - (b - a) * ((1.0 - u) * (1.0 - mode_fraction)).sqrt()
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = p.clamp(0.0, 1.0) * (sorted.len().saturating_sub(1) as f64);
    let low = rank.floor() as usize;
    let high = rank.ceil() as usize;
    if low == high {
        sorted[low]
    } else {
        let fraction = rank - low as f64;
        sorted[low] * (1.0 - fraction) + sorted[high] * fraction
    }
}

/// Sample triangular task durations and project complete baseline schedules.
pub fn simulate(plan: &Plan, config: SimulationConfig) -> Result<SimulationSummary, ScheduleError> {
    simulate_inner(plan, config, false)
}

/// Monte Carlo projection of remaining applicable work from the execution state at a clock reading.
///
/// It samples the same active graph as `deterministic_remaining`; excluded work has no criticality.
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
    let mut summary = simulate_inner(&remaining.plan, config, true)?;
    summary
        .criticality
        .retain(|id, _| !remaining.excluded.contains(id));
    Ok(summary)
}

/// Duration source of one activity across all iterations.
enum Activity {
    Fixed(f64),
    Sampled(dpm_model::ThreePointEstimate),
}

fn simulate_inner(
    plan: &Plan,
    config: SimulationConfig,
    remaining_only: bool,
) -> Result<SimulationSummary, ScheduleError> {
    plan.validate()?;
    if config.iterations == 0 {
        return Err(ScheduleError::NoSimulationIterations);
    }
    let network = Network::compile(plan)?;
    let mut rng = XorShift64::new(config.seed);
    let mut finishes = Vec::with_capacity(config.iterations);
    let mut critical_counts = vec![0_usize; network.order().len()];
    let mut durations = vec![0.0; network.order().len()];
    // Sampling follows work-item id order, so a seed draws the same numbers for the same plan.
    let mut sampled = Vec::with_capacity(plan.work_items.len());
    for (id, work) in &plan.work_items {
        let at = network
            .position(id)
            .ok_or(ScheduleError::MissingWorkItem(*id))?;
        let activity = if !work.is_executable()
            || (remaining_only && work.execution.status.satisfies_dependency())
        {
            Activity::Fixed(0.0)
        } else if let Some(estimate) = work.schedule.estimate {
            estimate
                .validate()
                .map_err(|_| ScheduleError::InvalidDuration(*id))?;
            Activity::Sampled(estimate)
        } else {
            Activity::Fixed(work.expected_duration_hours())
        };
        sampled.push((at, *id, activity));
    }
    for _ in 0..config.iterations {
        for (at, id, activity) in &sampled {
            durations[*at] = match activity {
                Activity::Fixed(hours) => *hours,
                Activity::Sampled(estimate) => sample_triangular(
                    &mut rng,
                    estimate.optimistic_hours,
                    estimate.likely_hours,
                    estimate.pessimistic_hours,
                ),
            };
            if !durations[*at].is_finite() || durations[*at] < 0.0 {
                return Err(ScheduleError::InvalidDuration(*id));
            }
        }
        let times = network.times(&durations)?;
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
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
