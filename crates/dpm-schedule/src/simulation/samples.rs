//! What a simulation records from each sampled schedule, and the summary it reduces to.
//!
//! Recording reads only projected times, never the generator, so adding a recorded quantity cannot
//! change which numbers a seed draws.

use super::{MilestoneFinish, SimulationSummary, percentile};
use crate::ScheduleError;
use crate::network::{EPSILON, Network, Times};
use dpm_model::WorkItemId;

/// Ascending finish samples in elapsed hours.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SortedSamples(Vec<f64>);

impl SortedSamples {
    pub(crate) fn new(mut values: Vec<f64>) -> Self {
        values.sort_by(f64::total_cmp);
        Self(values)
    }

    pub(crate) fn percentile(&self, p: f64) -> f64 {
        percentile(&self.0, p)
    }

    /// Fraction of samples at or before `horizon_hours`; 0 without samples.
    pub(crate) fn probability_by(&self, horizon_hours: f64) -> f64 {
        if self.0.is_empty() {
            return 0.0;
        }
        self.0.partition_point(|hours| *hours <= horizon_hours) as f64 / self.0.len() as f64
    }

    /// Sample standard deviation (n − 1 denominator); 0 for fewer than two samples.
    pub(crate) fn std_dev(&self) -> f64 {
        let count = self.0.len();
        if count < 2 {
            return 0.0;
        }
        let mean = self.0.iter().sum::<f64>() / count as f64;
        let squares: f64 = self
            .0
            .iter()
            .map(|hours| (hours - mean) * (hours - mean))
            .sum();
        (squares / (count - 1) as f64).sqrt()
    }
}

/// Per-sample observations of one simulation.
pub(crate) struct Samples {
    finishes: Vec<f64>,
    critical_counts: Vec<usize>,
    milestones: Vec<(WorkItemId, usize, Vec<f64>)>,
}

impl Samples {
    /// Room for `iterations` samples of `network`, recording the finish of each milestone at its
    /// topological position.
    pub(crate) fn new(
        network: &Network,
        iterations: usize,
        milestones: Vec<(WorkItemId, usize)>,
    ) -> Self {
        Self {
            finishes: Vec::with_capacity(iterations),
            critical_counts: vec![0; network.order().len()],
            milestones: milestones
                .into_iter()
                .map(|(id, at)| (id, at, Vec::with_capacity(iterations)))
                .collect(),
        }
    }

    /// Record one sampled schedule projected from `durations`.
    pub(crate) fn record(
        &mut self,
        network: &Network,
        durations: &[f64],
        times: &Times,
    ) -> Result<(), ScheduleError> {
        self.finishes.push(times.finish);
        for (at, count) in self.critical_counts.iter_mut().enumerate() {
            if network.total_float(times, at)? <= EPSILON {
                *count = count.wrapping_add(1);
            }
        }
        for (_, at, finishes) in &mut self.milestones {
            finishes.push(times.finishes(network, durations, *at)?.0);
        }
        Ok(())
    }

    pub(crate) fn summarize(self, network: &Network, iterations: usize) -> SimulationSummary {
        let finishes = SortedSamples::new(self.finishes);
        let criticality = network
            .order()
            .iter()
            .zip(self.critical_counts)
            .map(|(id, count)| (*id, count as f64 / iterations as f64))
            .collect();
        let milestones = self
            .milestones
            .into_iter()
            .map(|(id, _, samples)| (id, MilestoneFinish::new(SortedSamples::new(samples))))
            .collect();
        SimulationSummary {
            iterations,
            p50_finish_hours: finishes.percentile(0.50),
            p80_finish_hours: finishes.percentile(0.80),
            p95_finish_hours: finishes.percentile(0.95),
            criticality,
            sampled_std_dev_hours: finishes.std_dev(),
            milestones,
            finishes,
        }
    }
}
