//! Fractional sibling positions independent of task names and identifiers.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Lexicographic fractional position. Concurrent equal positions use the work ID as a tie-breaker.
/// Digits may contain zero internally; a nonzero final digit leaves room before every position.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SiblingOrder(pub Vec<u16>);

/// A requested insertion cannot be represented between its supplied bounds.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OrderError {
    /// Bounds must be valid and the left one strictly precede the right one.
    #[error("sibling order bounds must be valid and strictly increasing")]
    InvalidBounds,
    /// A pathological sequence of insertions exhausted the bounded portable representation.
    #[error("sibling order exceeds 128 digits")]
    Exhausted,
}

impl Default for SiblingOrder {
    fn default() -> Self {
        Self(vec![32768])
    }
}

impl SiblingOrder {
    /// Whether the position has a portable representation with room on either side.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.0.is_empty() && self.0.len() <= 128 && self.0.last() != Some(&0)
    }

    /// Allocate a position between two siblings without renumbering any existing position.
    /// An absent bound means the beginning or end of the sibling list.
    pub fn between(left: Option<&Self>, right: Option<&Self>) -> Result<Self, OrderError> {
        if left.is_some_and(|v| !v.is_valid())
            || right.is_some_and(|v| !v.is_valid())
            || left.zip(right).is_some_and(|(a, b)| a >= b)
        {
            return Err(OrderError::InvalidBounds);
        }
        let mut digits = Vec::new();
        let mut upper = right;
        for i in 0..128 {
            let low = left.and_then(|v| v.0.get(i)).copied().map_or(0, u32::from);
            let high = upper
                .and_then(|v| v.0.get(i))
                .copied()
                .map_or(65536, u32::from);
            if high > low + 1 {
                let digit = if right.is_none() && left.is_some() {
                    low + 1
                } else {
                    low + (high - low) / 2
                };
                digits.push(digit as u16);
                return Ok(Self(digits));
            }
            digits.push(low as u16);
            if low < high {
                upper = None;
            }
        }
        Err(OrderError::Exhausted)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
