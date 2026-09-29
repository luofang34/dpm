//! The clock reading queries are evaluated at.
//!
//! Gates that release after a lag, the remaining hours of started work and the `next` penalties
//! are measured from "now", so two reads of one revision differ by the time between them. Pinning
//! the reading makes such views reproducible for comparisons, replays and tests. Mutations never
//! read this clock: an operation's timestamp is when it was committed.

use super::Application;
use chrono::{DateTime, Utc};

/// Which reading of "now" queries use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QueryClock {
    /// The system clock, read once per query.
    #[default]
    System,
    /// A fixed instant, used by every query until changed.
    Fixed(DateTime<Utc>),
}

impl QueryClock {
    /// The reading one query is evaluated at.
    pub fn now(self) -> DateTime<Utc> {
        match self {
            Self::System => Utc::now(),
            Self::Fixed(at) => at,
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
}
