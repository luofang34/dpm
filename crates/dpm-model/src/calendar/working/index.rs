//! Constant-time rank queries on a sorted list of millisecond instants.
//!
//! Projections query calendars millions of times per simulation, so a binary search per query
//! dominates their cost. The index records, for every day of its range, how many values lie
//! before that day; a query starts there and steps over the few values inside one day. Values and
//! queries are integers, so a value on a bucket boundary ranks the same way in the index as in
//! the query.

/// Bucket width as a power of two, 2^26 ms (about 18.6 hours), so a bucket is a shift rather
/// than a division; it keeps the index small for century-long windows and a bucket's scan short.
const BUCKET_BITS: u32 = 26;
const BUCKET: i64 = 1 << BUCKET_BITS;

/// Sorted values with the rank of each bucket of their range.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Ranked {
    values: Vec<i64>,
    low: i64,
    /// Number of values below `low + bucket * BUCKET` for each bucket of the range.
    below: Vec<usize>,
}

impl Ranked {
    /// Index sorted `values` over `low..=high`.
    pub(super) fn new(values: Vec<i64>, low: i64, high: i64) -> Self {
        let buckets = usize::try_from(high.saturating_sub(low).max(0) / BUCKET).unwrap_or(0);
        let mut below = Vec::with_capacity(buckets.saturating_add(1));
        let mut count = 0;
        let mut at = low;
        for _ in 0..=buckets {
            while values.get(count).is_some_and(|value| *value < at) {
                count += 1;
            }
            below.push(count);
            at = at.saturating_add(BUCKET);
        }
        Self { values, low, below }
    }

    pub(super) fn get(&self, index: usize) -> Option<i64> {
        self.values.get(index).copied()
    }

    fn start(&self, t: i64) -> usize {
        let bucket = t.saturating_sub(self.low) >> BUCKET_BITS;
        let Ok(bucket) = usize::try_from(bucket) else {
            return 0;
        };
        let last = self.below.len().saturating_sub(1);
        self.below.get(bucket.min(last)).copied().unwrap_or(0)
    }

    /// Number of values below `t`.
    pub(super) fn below(&self, t: i64) -> usize {
        let mut count = self.start(t);
        while self.get(count).is_some_and(|value| value < t) {
            count += 1;
        }
        count
    }

    /// Number of values at or below `t`.
    pub(super) fn through(&self, t: i64) -> usize {
        let mut count = self.start(t);
        while self.get(count).is_some_and(|value| value <= t) {
            count += 1;
        }
        count
    }
}

#[cfg(test)]
mod tests;
