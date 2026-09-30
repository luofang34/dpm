//! Constant-time rank queries on a sorted list of hours.
//!
//! Projections query calendars millions of times per simulation, so a binary search per query
//! dominates their cost. The index records, for every whole hour of its range, how many values lie
//! before it; a query starts there and steps over the few values inside one hour.

/// Sorted values with the rank of each whole hour of their range.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Ranked {
    values: Vec<f64>,
    low: f64,
    /// Number of values below `low + hour` for each hour of the range.
    below: Vec<usize>,
}

impl Ranked {
    /// Index sorted `values` over `low..=high`.
    pub(super) fn new(values: Vec<f64>, low: f64, high: f64) -> Self {
        let hours = (high - low).max(0.0).ceil() as usize;
        let mut below = Vec::with_capacity(hours.saturating_add(1));
        let mut count = 0;
        for hour in 0..=hours {
            let at = low + hour as f64;
            while values.get(count).is_some_and(|value| *value < at) {
                count += 1;
            }
            below.push(count);
        }
        Self { values, low, below }
    }

    pub(super) fn get(&self, index: usize) -> Option<f64> {
        self.values.get(index).copied()
    }

    fn start(&self, t: f64) -> usize {
        let hour = (t - self.low).floor();
        if hour.is_nan() || hour < 0.0 {
            return 0;
        }
        let last = self.below.len().saturating_sub(1);
        let index = (hour as usize).min(last);
        self.below.get(index).copied().unwrap_or(0)
    }

    /// Number of values below `t`.
    pub(super) fn below(&self, t: f64) -> usize {
        let mut count = self.start(t);
        while self.get(count).is_some_and(|value| value < t) {
            count += 1;
        }
        count
    }

    /// Number of values at or below `t`.
    pub(super) fn through(&self, t: f64) -> usize {
        let mut count = self.start(t);
        while self.get(count).is_some_and(|value| value <= t) {
            count += 1;
        }
        count
    }
}

#[cfg(test)]
mod tests;
