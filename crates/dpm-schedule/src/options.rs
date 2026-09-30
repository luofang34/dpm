//! Opt-in adjustments to a remaining projection that callers request explicitly.

use crate::ScheduleError;
use serde::{Deserialize, Serialize};

/// Adjustments a caller applies to one remaining projection; the default changes nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct RemainingOptions {
    /// Elapsed hours every task still awaiting verification waits for review after its work
    /// ends, before the verifier's calendar is consulted. Zero reproduces the plain projection.
    pub review_delay_hours: f64,
}

impl RemainingOptions {
    /// Reject a review delay that is negative or not finite.
    pub fn validate(self) -> Result<(), ScheduleError> {
        if self.review_delay_hours.is_finite() && self.review_delay_hours >= 0.0 {
            Ok(())
        } else {
            Err(ScheduleError::InvalidReviewDelay(self.review_delay_hours))
        }
    }

    /// Whether the projection needs a review delay at all.
    pub(crate) fn delays_review(self) -> bool {
        self.review_delay_hours > 0.0
    }
}
