//! The local native boundary: a versioned request/response contract over the shared application.
//!
//! A macOS client (or any local native host) never reimplements rules. It sends one JSON request
//! per call and receives one JSON response, and every answer is computed by the same
//! [`Application`] queries and commands the CLI and agent tools use. This module adds only what a
//! long-lived native client needs and the one-shot adapters do not:
//!
//! - **negotiation** of a protocol version and a statement of capabilities and unsupported
//!   controls ([`NativeCall::Hello`]);
//! - an **attachment** naming the workspace and lineage a client's state belongs to
//!   ([`NativeCall::Attach`]), checked on every later call, so a restored copy or a repointed
//!   source is refused rather than silently followed;
//! - a **basis** on every answer ([`ViewBasis`]): where each store the answer read stood just
//!   before it was read, one position per store and never one for a store the answer did not read.
//!   A basis is a lower bound, so a subscription from it misses nothing; the answer may already
//!   hold some of what follows, so a poll may repeat it. Reads never wait for writes to stop, so
//!   continuous run telemetry cannot starve a project query or a run query;
//! - **independent cursors** for the three feeds (project operations, run lifecycle, run activity)
//!   and an invalidation mark for run operation links, polled together and boundedly
//!   ([`NativeCall::Changes`]), with resets and retention gaps reported explicitly. Every durable
//!   fact a run's view is built from is tracked, so two equal positions in one epoch never describe
//!   different run facts. Links are the one fact with no resumable position: the mark only says
//!   that some run's operations changed, and the client reads its displayed runs again.
//!
//! A consumer follows two rules throughout. Nothing advances until it has been applied: a feed
//! cursor moves only after its page was applied, so a lost response repeats a page and never skips
//! one, and a repeat of a page already applied is discarded by feed identity (lineage or epoch) and
//! sequence, with an explicit reset when that identity changes. A view that any signal invalidated
//! (a feed entry, a gap, a changed link count, a reset) stays marked stale until a view read after
//! that signal has been installed, and only then does the client adopt that view's basis, for the
//! stores the view carries, as the position it is current at. A failed refresh or a dropped
//! connection therefore leaves the staleness visible to the next poll. The project store and the
//! run store are separate files read in separate transactions: no position claims one atomic
//! snapshot of both.
//!
//! The boundary is transport-neutral: [`Application::native_json_blocking`] maps one request line
//! to one response line, and a host decides whether that runs over a pipe to a helper process or
//! through a foreign-function call. It holds no state per client, so a slow or disconnected
//! consumer costs the producer nothing and recovers from its cursors.

mod error;
mod feeds;
mod handler;
mod protocol;
mod source;
mod views;

pub use error::{NativeError, NativeErrorBody};
pub use protocol::{
    Attached, Attachment, Capabilities, ChangesResult, Committed, Cursors, FeedCursor, FeedDelta,
    FeedStatus, HelloResult, Limits, LinkMark, LinkSignal, NATIVE_PROTOCOL_VERSION,
    NATIVE_PROTOCOLS, NativeCall, NativeEnvelope, NativeRequest, NativeResponse, NativeResult,
    ProjectCursor, ProjectWatermark, ResetReason, RunCapabilities, SourceChange, SourceIdentity,
    SourceKind, UnsupportedControl, View, ViewBasis, Watermark,
};
pub use views::{ViewMapping, view_mappings};

#[cfg(test)]
#[cfg(feature = "sqlite")]
mod tests;
