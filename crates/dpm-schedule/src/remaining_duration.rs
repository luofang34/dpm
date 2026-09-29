//! One activity's remaining duration: the value CPM projects and the distribution Monte Carlo
//! samples, built together so the deterministic value is always the mean of the samples.
//!
//! A task that has not started contributes its whole beta-PERT duration, whose mean is the PERT
//! expectation. A task with a recorded start event contributes only what remains after the elapsed
//! hours since that start, sampled conditional on the task not having finished yet: a duration
//! that the elapsed time already rules out is never drawn. Its deterministic value is the mean of
//! that same conditional distribution rather than `expected - elapsed`, which would reach zero
//! while the estimate still gives the task a substantial chance of running on, and would then
//! release successors earlier than the simulation expects.

use crate::ScheduleError;
use crate::sampling::{BetaPert, XorShift64};
use dpm_model::{WorkItem, WorkItemId};

/// Cells of the tabulated conditional density.
///
/// The beta-PERT density is bounded, so a midpoint table this fine keeps the conditional mean
/// within a small fraction of a percent of the range while costing a few kilobytes per started
/// task.
const CELLS: usize = 1024;

/// Duration source of one activity for every projection of the same plan snapshot.
#[derive(Debug, Clone)]
pub(crate) enum RemainingDuration {
    /// Known remaining hours: completed, unestimated, exactly estimated, overrun or aggregate work.
    Fixed(f64),
    /// The whole beta-PERT duration less elapsed hours that the optimistic bound already covers.
    Shifted {
        /// Distribution of the task's total duration.
        distribution: BetaPert,
        /// PERT expectation of the total duration.
        expected: f64,
        /// Elapsed hours since the start event, at most the optimistic bound.
        elapsed: f64,
    },
    /// Remaining hours of a task whose elapsed time rules out part of its estimate.
    Truncated(Truncated),
}

/// Beta-PERT density restricted to durations longer than the elapsed time, tabulated on a grid.
///
/// Sampling inverts the table's piecewise-uniform distribution with one uniform draw, and the
/// deterministic value is that same distribution's mean, so the two cannot disagree.
#[derive(Debug, Clone)]
pub(crate) struct Truncated {
    distribution: BetaPert,
    elapsed: f64,
    /// Fraction of the estimate's range the elapsed time already covers, in (0, 1).
    from: f64,
    /// Width of one cell as a fraction of the range.
    width: f64,
    /// Running mass at each cell boundary, starting at zero.
    cumulative: Vec<f64>,
    /// Conditional mean fraction of the range.
    mean_fraction: f64,
}

impl RemainingDuration {
    /// The duration source of `work`, whose start event lies `elapsed` hours before the origin.
    ///
    /// `done` work contributes nothing. Elapsed time includes every blocked interval, because
    /// estimates are elapsed hours and not effort. Work whose start time was never recorded is
    /// passed an elapsed time of zero and keeps its whole duration.
    pub(crate) fn of(
        id: WorkItemId,
        work: &WorkItem,
        done: bool,
        elapsed: f64,
    ) -> Result<Self, ScheduleError> {
        if !work.is_executable() || done {
            return Ok(Self::Fixed(0.0));
        }
        let Some(estimate) = work.schedule.estimate else {
            return Ok(Self::Fixed(work.expected_duration_hours()));
        };
        estimate
            .validate()
            .map_err(|_| ScheduleError::InvalidDuration(id))?;
        if !elapsed.is_finite() {
            return Err(ScheduleError::InvalidDuration(id));
        }
        let elapsed = elapsed.max(0.0);
        let expected = estimate.pert_expected_hours();
        let Some(distribution) = BetaPert::new(estimate) else {
            return Ok(Self::Fixed((expected - elapsed).max(0.0)));
        };
        let from = distribution.fraction_of(elapsed);
        Ok(if from <= 0.0 {
            Self::Shifted {
                distribution,
                expected,
                elapsed,
            }
        } else if from >= 1.0 {
            // The task has outlasted its pessimistic bound, so the estimate says nothing about
            // what remains. It projects to finish at the origin: no earlier than execution can
            // release its successors, which still wait for its verification.
            Self::Fixed(0.0)
        } else {
            Self::Truncated(Truncated::new(distribution, elapsed, from))
        })
    }

    /// Remaining hours used by the deterministic projection: the mean of `sample`.
    pub(crate) fn expected(&self) -> f64 {
        match self {
            Self::Fixed(hours) => *hours,
            Self::Shifted {
                expected, elapsed, ..
            } => (expected - elapsed).max(0.0),
            Self::Truncated(table) => table.remaining_at(table.mean_fraction),
        }
    }

    /// One sampled remaining duration in elapsed hours.
    pub(crate) fn sample(&self, rng: &mut XorShift64) -> f64 {
        match self {
            Self::Fixed(hours) => *hours,
            Self::Shifted {
                distribution,
                elapsed,
                ..
            } => (distribution.sample(rng) - elapsed).max(0.0),
            Self::Truncated(table) => table.sample(rng),
        }
    }
}

impl Truncated {
    fn new(distribution: BetaPert, elapsed: f64, from: f64) -> Self {
        let width = (1.0 - from) / CELLS as f64;
        let mut masses: Vec<f64> = (0..CELLS)
            .map(|cell| distribution.relative_density(from + width * (cell as f64 + 0.5)))
            .collect();
        let total: f64 = masses.iter().sum();
        if !(total.is_finite() && total > 0.0) {
            // Unreachable for shapes in [1, 5] away from the range's ends; a uniform remainder is
            // the least informative distribution on what the estimate still allows.
            masses.fill(1.0);
        }
        let mut cumulative = Vec::with_capacity(CELLS + 1);
        let mut running = 0.0;
        let mut moment = 0.0;
        cumulative.push(running);
        for (cell, mass) in masses.iter().enumerate() {
            running += mass;
            moment += mass * (from + width * (cell as f64 + 0.5));
            cumulative.push(running);
        }
        Self {
            distribution,
            elapsed,
            from,
            width,
            cumulative,
            mean_fraction: moment / running,
        }
    }

    fn remaining_at(&self, fraction: f64) -> f64 {
        (self.distribution.hours_at(fraction) - self.elapsed).max(0.0)
    }

    fn sample(&self, rng: &mut XorShift64) -> f64 {
        let total = self.cumulative.last().copied().unwrap_or(0.0);
        let target = rng.unit_f64() * total;
        let cell = self
            .cumulative
            .partition_point(|mass| *mass <= target)
            .saturating_sub(1)
            .min(CELLS - 1);
        let low = self.cumulative[cell];
        let mass = self.cumulative[cell + 1] - low;
        let within = if mass > 0.0 {
            ((target - low) / mass).clamp(0.0, 1.0)
        } else {
            0.5
        };
        self.remaining_at(self.from + self.width * (cell as f64 + within))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
