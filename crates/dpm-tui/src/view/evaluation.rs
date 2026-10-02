//! When the displayed projections stop holding although the snapshot has not changed.
//!
//! Readiness, remaining hours and forecasts are evaluated at one clock reading. A gate whose
//! elapsed lag opens later, or started work whose remaining hours shrink, changes them without a
//! new revision, so no source probe notices. The view re-evaluates its own snapshot at the
//! earliest pending release, and at least every [`MAX_AGE`] for continuously drifting forecasts;
//! between those instants a check is one comparison, so activity never reruns the simulation.

use super::View;
use chrono::{DateTime, TimeDelta, Utc};
use dpm_model::{Plan, Timeline};

/// Longest a projection stays displayed without re-evaluation, which bounds how often the
/// probabilistic forecast reruns while nothing else changes.
pub(crate) const MAX_AGE: TimeDelta = TimeDelta::seconds(60);

/// Clock readings within which the displayed projections are current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Window {
    /// Earliest valid reading; an earlier one means the clock stepped back.
    from: DateTime<Utc>,
    /// First reading at which the projections must be evaluated again.
    until: DateTime<Utc>,
}

impl Window {
    /// The window of projections evaluated at `clock` over `timeline`.
    pub(crate) fn evaluated(plan: &Plan, timeline: &Timeline, clock: DateTime<Utc>) -> Self {
        let aged = clock.checked_add_signed(MAX_AGE).unwrap_or(clock);
        // A choice recorded with a future time is not reported as elapsing, so it is picked up by
        // the `MAX_AGE` bound instead.
        let until = timeline
            .next_release(plan)
            .map_or(aged, |opens| opens.min(aged));
        Self { from: clock, until }
    }

    /// A window after a failed evaluation at `now`: retried after [`MAX_AGE`], not every frame.
    fn retry(now: DateTime<Utc>) -> Self {
        Self {
            from: now,
            until: now.checked_add_signed(MAX_AGE).unwrap_or(now),
        }
    }

    fn holds_at(self, now: DateTime<Utc>) -> bool {
        self.from <= now && now < self.until
    }
}

impl View {
    /// Re-evaluate the displayed snapshot at `now` when its projections no longer hold there;
    /// `true` when it did. The snapshot, navigation and any source notice stay as they are: a
    /// clock change says nothing about the source.
    pub(crate) fn reevaluate_if_due(&mut self, now: DateTime<Utc>) -> bool {
        if self.window.holds_at(now) {
            return false;
        }
        let notice = self.notice.take();
        let plan = self.plan.clone();
        let result = self.rebuild(&plan, now);
        self.notice = notice;
        match result {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, evaluated_at = %self.clock, "re-evaluating the snapshot failed");
                self.window = Window::retry(now);
                false
            }
        }
    }

    /// Clock reading every displayed projection was evaluated at.
    pub(crate) fn evaluated_at(&self) -> DateTime<Utc> {
        self.clock
    }

    /// First clock reading at which the displayed projections are evaluated again.
    #[cfg(test)]
    pub(crate) fn evaluation_due(&self) -> DateTime<Utc> {
        self.window.until
    }
}
