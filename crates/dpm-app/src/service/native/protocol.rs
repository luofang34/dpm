//! The request, response and value types of the native boundary.

use super::{error::NativeErrorBody, views::ViewMapping};
use crate::{CommandRequest, Query, QueryResponse};
use chrono::{DateTime, Utc};
use dpm_model::{ActivityEntry, ActivityGap, LifecycleEntry, LineageId, RunFeedHeads, WorkspaceId};
use dpm_store::{HistoryEntry, RecordedOperation};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version this build speaks.
pub const NATIVE_PROTOCOL_VERSION: u32 = 1;

/// Every protocol version this build speaks, ascending.
pub const NATIVE_PROTOCOLS: [u32; 1] = [NATIVE_PROTOCOL_VERSION];

/// One request line. `id` is the client's own correlation string, echoed in the response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRequest {
    /// The protocol version the client speaks; `hello` negotiates it.
    pub protocol: u32,
    /// Correlation identifier chosen by the client.
    pub id: String,
    /// What is being asked.
    pub call: NativeCall,
}

/// A call a native client may make.
///
/// Every read is an existing shared [`Query`]; the only mutation is the existing
/// [`CommandRequest`], whose revision, lineage, ownership and validation rules are the
/// application's own, so a preview or an archive refuses it exactly as the CLI does.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeCall {
    /// Negotiate a protocol version and read the capabilities.
    Hello {
        /// Every protocol version the client can speak.
        protocols: Vec<u32>,
    },
    /// Establish what the client's state belongs to and where every feed stands now.
    Attach {
        /// The workspace the client expects; another workspace is refused.
        #[serde(default)]
        expect_workspace: Option<WorkspaceId>,
    },
    /// A shared read-only query, answered with the envelope the CLI and tools return, plus the
    /// watermark it was computed at.
    Query {
        /// The shared query.
        query: Query,
        /// What the client is attached to; a different workspace or lineage is refused.
        #[serde(default)]
        attached: Option<Attachment>,
    },
    /// Everything that changed after the client's cursors, in independent, bounded feeds.
    Changes {
        /// Where the client stands in each feed it follows.
        since: Cursors,
        /// Most entries per feed, default 100, at most 1000.
        #[serde(default)]
        limit: Option<u16>,
        /// What the client is attached to; a different workspace is refused.
        #[serde(default)]
        attached: Option<Attachment>,
    },
    /// A shared project mutation, with the application's own preconditions.
    ///
    /// The source guard runs first: a connection whose locator now selects another source refuses
    /// to write the one it opened.
    Command {
        /// The command exactly as the CLI and tools build it.
        request: CommandRequest,
        /// What the client is attached to; a different workspace or lineage is refused.
        #[serde(default)]
        attached: Option<Attachment>,
    },
}

/// The workspace and history a client's state belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Workspace identity.
    pub workspace_id: WorkspaceId,
    /// Writable history the client's revision is meaningful in; absent for a read-only preview.
    #[serde(default)]
    pub lineage_id: Option<LineageId>,
}

/// Where the project's operation history stands.
///
/// Revisions wrap, so two watermarks are compared only for equality, never for order. The
/// history head is a local append counter and is the cursor to follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectWatermark {
    /// Workspace identity.
    pub workspace_id: WorkspaceId,
    /// Lineage the revision belongs to; absent for a preview.
    #[serde(default)]
    pub lineage_id: Option<LineageId>,
    /// Committed plan revision.
    pub revision: u64,
    /// Local sequence of the newest committed operation, read in the same transaction as the
    /// revision, so a subscription from here misses no change and repeats none.
    pub history_head: u64,
}

/// Where the project and the run feeds stand, for seeding a client and for the end of a poll.
///
/// The project store and the run store are separate files. Each half is read in its own
/// transaction, so a watermark never claims the two form one atomic snapshot; it states where each
/// feed stood when it was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watermark {
    /// The project's operation history.
    pub project: ProjectWatermark,
    /// The run lifecycle and activity feeds.
    pub runs: RunFeedHeads,
}

/// What kind of source the boundary is attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A read-only plan file: every mutation is refused and there are no feeds.
    Preview,
    /// A writable local store.
    Live,
    /// A backup archive: readable, never written.
    Archive,
}

/// What a source is, as far as a client's state depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceIdentity {
    /// Preview, live store or archive.
    pub kind: SourceKind,
    /// Workspace identity when it was read; a store is identified by its lineage and leaves this
    /// absent.
    #[serde(default)]
    pub workspace_id: Option<WorkspaceId>,
    /// Writable history; absent for a preview.
    #[serde(default)]
    pub lineage_id: Option<LineageId>,
}

/// How the source a locator selects differs from the one a connection opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceChange {
    /// The locator names another file than the one opened, even if it holds the same identity.
    Locator,
    /// The source is now a different kind, such as a preview where a store was.
    Kind,
    /// The same file now holds another workspace.
    Workspace,
    /// The store now continues another history, such as after a restore.
    Lineage,
    /// The locator now names another workspace asset, which changes the context queries are
    /// scoped to; a connection keeps the one it opened with.
    Asset,
}

impl std::fmt::Display for SourceChange {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Locator => "the locator names another file",
            Self::Kind => "the source is another kind",
            Self::Workspace => "the file holds another workspace",
            Self::Lineage => "the store continues another history",
            Self::Asset => "the locator names another workspace asset",
        })
    }
}

/// A response envelope in the shape the CLI `--json` output and MCP `structuredContent` use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeEnvelope<T> {
    /// Application wire contract version.
    pub api_version: u32,
    /// Revision the result observed or produced.
    pub revision: Option<u64>,
    /// Lineage of that revision.
    pub lineage_id: Option<LineageId>,
    /// The result.
    pub data: T,
}

impl From<QueryResponse> for NativeEnvelope<Value> {
    fn from(response: QueryResponse) -> Self {
        Self {
            api_version: response.api_version,
            revision: Some(response.revision),
            lineage_id: response.lineage_id,
            data: response.data,
        }
    }
}

/// One response line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeResponse {
    /// The protocol version the answer is in.
    pub protocol: u32,
    /// The request's correlation identifier; empty when the request was too malformed to carry one.
    pub id: String,
    /// Whether the call succeeded; exactly one of `result` and `error` is present.
    pub ok: bool,
    /// The result of a successful call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<NativeResult>,
    /// The typed refusal of a failed call, in the shape CLI and tool errors use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<NativeErrorBody>,
}

/// The result of a successful call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NativeResult {
    /// Answer to `hello`.
    Hello(HelloResult),
    /// Answer to `attach`.
    Attached(Attached),
    /// Answer to `query`.
    View(View),
    /// Answer to `changes`.
    Changes(ChangesResult),
    /// Answer to `command`.
    Committed(Committed),
}

/// Negotiated protocol and what the host supports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloResult {
    /// The protocol version chosen: the highest both sides speak.
    pub protocol: u32,
    /// Every version this build speaks.
    pub supported: Vec<u32>,
    /// The application wire contract version of the envelopes inside results.
    pub api_version: u32,
    /// What the host can do and what it explicitly cannot.
    pub capabilities: Capabilities,
}

/// What the host supports, so a client degrades visibly instead of guessing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    /// Each macOS view and the shared queries and feeds that feed it.
    pub views: Vec<ViewMapping>,
    /// Run observation, including what an unmanaged session means.
    pub runs: RunCapabilities,
    /// Whether project commands are accepted, and the preconditions they carry.
    pub commands: bool,
    /// Controls that this boundary does not offer.
    pub unsupported: Vec<UnsupportedControl>,
    /// Bounds a client can rely on.
    pub limits: Limits,
}

/// How runs are observed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCapabilities {
    /// Whether this build keeps a run store at all.
    pub supported: bool,
    /// The observation modes a run can have.
    pub observation: Vec<String>,
    /// What `reported_only` means: silence from such a session is stale, never idle or finished.
    pub reported_only: String,
    /// A completed run is an executor's report, not accepted work.
    pub completion: String,
}

/// A control the boundary does not offer, and why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsupportedControl {
    /// Stable name of the control.
    pub control: String,
    /// The roadmap contract that would add it.
    pub reason: String,
}

/// Bounds a client can rely on.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Limits {
    /// Most entries one feed page returns.
    pub max_page: u16,
    /// Page size when a request names none.
    pub default_page: u16,
    /// Seconds without a receipt after which an unfinished run is stale.
    pub stale_after_seconds: u32,
    /// How often a client should re-evaluate time-dependent views at most, as a ceiling; a
    /// result's `refresh_at` names an earlier instant when one is known.
    pub reevaluate_within_seconds: u32,
}

/// Answer to `attach`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attached {
    /// What the client's state now belongs to.
    pub attachment: Attachment,
    /// Whether the source is a preview, a live store or an archive.
    pub source: SourceKind,
    /// Where every feed stands; the seed for the client's cursors.
    pub watermark: Watermark,
    /// The clock reading of this answer.
    pub evaluated_at: DateTime<Utc>,
}

/// The positions an answer is anchored to, one per store it read, each independent of the other.
///
/// A basis is a lower bound, read just before its store was read: everything after it is still to
/// come, so a subscription from it misses nothing, and the answer may already hold some of what
/// follows, so it may repeat it. A consumer takes each position only from an answer that carries
/// it. A project-only answer has no run basis, so a later project-only response can never move a
/// run cursor past a change its held run snapshot lacks. The two stores are separate files read in
/// separate transactions, so no basis claims one atomic snapshot of both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewBasis {
    /// The project's operation history as it stood just before the project was read; present on
    /// every answer. The answer's own `revision` is the one it was computed from, which may be
    /// later than this position when a commit landed in between.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectWatermark>,
    /// The run feeds and link count; present only when the query reads runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runs: Option<RunFeedHeads>,
}

/// Answer to `query`: the shared envelope plus the identity needed to follow it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    /// The one clock reading every time-dependent value in `envelope` was evaluated at.
    pub evaluated_at: DateTime<Utc>,
    /// Where each store this answer read stood just before it was read.
    pub basis: ViewBasis,
    /// The first instant at which a time-dependent value in `envelope` changes with no new
    /// revision, when the answer knows one: a run turning stale, or the shared gate evaluator's
    /// earliest pending dependency lag opening. A client re-queries at this instant and never
    /// derives readiness itself. Absent means no such instant is known; probabilistic forecasts
    /// still drift, so a client also re-evaluates within `limits.reevaluate_within_seconds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_at: Option<DateTime<Utc>>,
    /// The same envelope the CLI `--json` output and MCP `structuredContent` carry for this query.
    pub envelope: NativeEnvelope<Value>,
}

/// Answer to `command`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Committed {
    /// The recorded operation in the CLI and tool envelope.
    pub envelope: NativeEnvelope<RecordedOperation>,
}

/// The client's position in the project's operation history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCursor {
    /// The lineage the cursor was issued under.
    #[serde(default)]
    pub lineage_id: Option<LineageId>,
    /// Exclusive local sequence to continue after.
    pub after_sequence: u64,
}

/// The client's position in a run feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedCursor {
    /// The run store epoch the cursor was issued under.
    #[serde(default)]
    pub epoch: Option<LineageId>,
    /// Exclusive feed sequence to continue after.
    pub after_sequence: u64,
}

/// Where a client stands in each feed. Each is independent; omit a feed to not follow it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursors {
    /// Project operations.
    #[serde(default)]
    pub project: Option<ProjectCursor>,
    /// Run lifecycle facts.
    #[serde(default)]
    pub lifecycle: Option<FeedCursor>,
    /// Run activity.
    #[serde(default)]
    pub activity: Option<FeedCursor>,
    /// Operation links: an association recorded after the fact changes what a run's view lists
    /// without being a transition or activity, so it is tracked on its own. It is an invalidation
    /// mark, not a feed cursor.
    #[serde(default)]
    pub links: Option<LinkMark>,
}

/// What a client has seen of the operation links: how many existed, in which epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkMark {
    /// The run store epoch the count was read under.
    #[serde(default)]
    pub epoch: Option<LineageId>,
    /// The `count` a previous answer reported.
    pub count: u64,
}

/// The state of the operation-link invalidation token.
///
/// Links are append-only, so the count only grows within an epoch. The token says that some run's
/// linked operations changed, not which and not in what order: it is a bounded re-read signal and
/// offers no resumable position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkSignal {
    /// Whether the mark could be compared; a restore or rollback is a reset.
    #[serde(flatten)]
    pub status: FeedStatus,
    /// Whether links were recorded since the mark. When true the client keeps the run views it
    /// displays marked stale until it has read them again and installed that answer, and only
    /// then adopts that answer's `watermark.runs.link_count` as its mark. Adopting `count` before a
    /// successful re-read would hide the staleness from the next poll if the re-read failed or the
    /// connection dropped.
    pub changed: bool,
    /// The link count this answer was computed at: always the `link_count` of the watermark in the
    /// same answer. It says what a re-read will at least reflect; it is not a mark to keep until
    /// that re-read has been installed.
    pub count: u64,
}

/// Why a cursor can no longer be followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetReason {
    /// The project's lineage is not the one the cursor was issued under, such as after a restore.
    LineageChanged,
    /// The run store's epoch changed, such as a restored run store.
    EpochChanged,
    /// The cursor is beyond the feed's head: the source holds an older history than the client saw.
    CursorAhead,
}

/// Whether a feed can be followed from the client's cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FeedStatus {
    /// The cursor is valid and `entries` continue it.
    Continue,
    /// The cursor cannot be followed. No entries are returned; the client discards what it held
    /// for this feed and re-seeds from a fresh `attach` and queries.
    Reset {
        /// Why.
        reason: ResetReason,
    },
}

/// One bounded page of a feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedDelta<T> {
    /// Whether the cursor could be followed.
    #[serde(flatten)]
    pub status: FeedStatus,
    /// Entries in feed order, at most the requested limit.
    pub entries: Vec<T>,
    /// The cursor to continue after; the client's own cursor on a reset.
    pub next_after_sequence: u64,
    /// Whether the feed held more than this page: false means the client is caught up at the
    /// watermark of this answer.
    pub more: bool,
    /// Present on an activity page when retention removed records the cursor had not read; the
    /// client treats the feed as discontinuous and re-reads the affected runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<ActivityGap>,
}

/// Answer to `changes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesResult {
    /// The clock reading of this answer. A client that sees no entries still compares it with the
    /// `refresh_at` of its views: time passing is a change no feed reports.
    pub evaluated_at: DateTime<Utc>,
    /// Where every feed stood after the pages were read; never older than the pages.
    pub watermark: Watermark,
    /// Project operations after the project cursor, or `None` when not followed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<FeedDelta<HistoryEntry>>,
    /// Run lifecycle facts after the lifecycle cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<FeedDelta<LifecycleEntry>>,
    /// Run activity after the activity cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<FeedDelta<ActivityEntry>>,
    /// Whether operation links changed since the link mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<LinkSignal>,
}
