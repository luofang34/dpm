//! The persistent local helper behind the native DPM contract.
//!
//! One process owns one workspace through [`dpm_app::Application`] and answers newline-framed JSON
//! requests on standard input with one response line each on standard output. It adds only what a
//! long-lived local transport needs around the accepted native contract: bounded frames in both
//! directions, correlated and serialized exchanges on one serving loop that alone owns the
//! application and store, structured startup refusals, and explicit exits. Standard output carries protocol frames only; diagnostics go to standard
//! error. It holds no per-client state, so a client that loses it starts another and continues from
//! its own cursors.

mod frame;
mod host;
mod limits;
mod options;
mod rejection;
mod start;

pub use frame::{Frame, FrameFault, FrameReader, Next};
pub use host::{Ending, HostError, serve_blocking};
pub use limits::{
    DEFAULT_MAX_REQUEST_BYTES, DEFAULT_MAX_RESPONSE_BYTES, ID_ALPHABET, LimitError, Limits,
    MAX_FRAME_BYTES, MAX_ID_BYTES, MIN_FRAME_BYTES,
};
pub use options::{Options, OptionsError, Parsed, USAGE};
pub use rejection::codes;
pub use start::{StartError, start_blocking};
