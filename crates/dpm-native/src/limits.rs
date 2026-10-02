//! The frame bounds, validated so that every frame the helper sends can obey them.
//!
//! A bound too small to hold the helper's own refusals would force it to break the bound to say
//! why, so such a bound cannot be constructed. The refusals are bounded by construction: a
//! correlation identifier is at most [`MAX_ID_BYTES`], messages and details are fixed text and
//! numbers, and the smallest accepted bound is several times the largest refusal.

use thiserror::Error;

/// Largest request frame accepted when the command line names no bound.
pub const DEFAULT_MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

/// Largest response frame sent when the command line names no bound.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

/// The smallest bound either direction accepts.
pub const MIN_FRAME_BYTES: usize = 1024;

/// A bound above this is not a bound.
pub const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;

/// Longest correlation identifier a request may carry. A longer one is refused before the request
/// is processed, so a refusal never has to echo an identifier it cannot fit.
pub const MAX_ID_BYTES: usize = 128;

/// The characters of a correlation identifier, none of which needs a JSON escape.
pub const ID_ALPHABET: &str = "A-Z a-z 0-9 . _ : -";

/// A bound outside what the helper can honour.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("{name} must be between {MIN_FRAME_BYTES} and {MAX_FRAME_BYTES} bytes, not {value}")]
pub struct LimitError {
    /// Which bound.
    pub name: &'static str,
    /// What was given.
    pub value: usize,
}

/// Largest frame, in bytes, in each direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    max_request_bytes: usize,
    max_response_bytes: usize,
}

fn check(name: &'static str, value: usize) -> Result<usize, LimitError> {
    if (MIN_FRAME_BYTES..=MAX_FRAME_BYTES).contains(&value) {
        Ok(value)
    } else {
        Err(LimitError { name, value })
    }
}

impl Limits {
    /// Validated bounds for a request line and for a response line.
    pub fn new(max_request_bytes: usize, max_response_bytes: usize) -> Result<Self, LimitError> {
        Ok(Self {
            max_request_bytes: check("the request bound", max_request_bytes)?,
            max_response_bytes: check("the response bound", max_response_bytes)?,
        })
    }

    /// Longest request line accepted; a longer one is refused without being buffered.
    #[must_use]
    pub fn max_request_bytes(self) -> usize {
        self.max_request_bytes
    }

    /// Longest response line sent; a longer answer is replaced by a typed refusal, never cut.
    #[must_use]
    pub fn max_response_bytes(self) -> usize {
        self.max_response_bytes
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }
}
