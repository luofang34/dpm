//! The clock reading queries are evaluated at.
//!
//! Gates that release after a lag, the remaining hours of started work and the `next` penalties
//! are measured from "now", so two reads of one revision differ by the time between them. Pinning
//! the reading makes such views reproducible for comparisons, replays and tests. Mutations never
//! read this clock: an operation's timestamp is when it was committed.

use super::Application;
use crate::AppError;
use chrono::{DateTime, Utc};
use dpm_model::Plan;

/// Which reading of "now" queries use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QueryClock {
    /// The system clock, read once per query.
    #[default]
    System,
    /// A fixed instant, used by every query until changed.
    Fixed(DateTime<Utc>),
    /// A caller's fixed instant, at which a query observes the workspace as it stood then: the
    /// current snapshot already holds later events, which the view withdraws.
    Pinned(DateTime<Utc>),
}

impl QueryClock {
    /// The reading one query is evaluated at.
    pub fn now(self) -> DateTime<Utc> {
        match self {
            Self::System => Utc::now(),
            Self::Fixed(at) | Self::Pinned(at) => at,
        }
    }
}

impl Application {
    /// Evaluate later queries at this clock; operation timestamps are unaffected.
    pub fn set_query_clock(&mut self, clock: QueryClock) {
        self.clock = clock;
    }
    /// This application with queries evaluated at `clock`.
    #[must_use]
    pub fn with_query_clock(mut self, clock: QueryClock) -> Self {
        self.clock = clock;
        self
    }
    /// The clock queries are evaluated at.
    pub fn query_clock(&self) -> QueryClock {
        self.clock
    }
    /// The plan a query observes and the reading it is evaluated at.
    ///
    /// At a caller's pinned reading the plan holds only the execution facts that had occurred by
    /// then ([`Plan::as_of`]), judged by their event times, so work recorded after the fact or
    /// imported with its history looks back to when it happened. The operation log, not this
    /// view, shows what had been recorded by an instant.
    pub(super) fn observed_plan_blocking(&self) -> Result<(Plan, DateTime<Utc>), AppError> {
        let plan = self.plan_blocking()?;
        Ok(match self.clock {
            QueryClock::Pinned(at) => (plan.as_of(at), at),
            clock => (plan, clock.now()),
        })
    }
}

#[cfg(test)]
#[cfg(feature = "sqlite")]
mod tests;
