//! The immutable facts a run records when it starts.

use crate::{
    ActorId, ArtifactId, AssetId, Key, LineageId, OperationId, RunId, WorkContract, WorkItemId,
    WorkspaceId,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The external runtime session a run belongs to, named by the provider's own identifiers.
///
/// Identifiers are opaque to DPM and are never a place for secrets: they appear in shared views.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSession {
    /// Lowercase provider word such as `codex` or `claude`; DPM keeps no table of providers.
    pub provider: String,
    /// The provider's session or thread identifier.
    pub session: String,
    /// The provider's turn or request identifier within the session, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    /// The runtime's identity and configuration as the recorder attests them when the run starts.
    /// Fixed for the life of the run and compared when a start is resent. Boxed so that a start
    /// request without it stays small.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Box<RunProvenance>>,
}

/// What the recorder attests about the runtime a run executed in, fixed when the run starts.
///
/// The requested and the observed model are kept apart: the first is what the recorder asked the
/// runtime to use, the second what the runtime announced it used. Neither is a place for secrets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunProvenance {
    /// The model the recorder asked for; absent when it left the choice to the runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    /// The model the runtime announced, when it announced one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_model: Option<String>,
    /// The runtime's version as it announced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    /// SHA-256, in lowercase hex, of the canonical public configuration the runtime was started
    /// with: permission mode, tools, allowed rules and confinement. The configuration itself is
    /// not stored, only what identifies it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_digest: Option<String>,
}

/// How completely DPM observes the executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Observation {
    /// A service hosts or watches the executor, so silence is evidence of a problem.
    Managed,
    /// The executor reports itself through the CLI or agent tools; absent reports prove nothing,
    /// so a silent run is stale, never idle or finished.
    ReportedOnly,
}

impl Observation {
    /// The lowercase word every adapter spells the mode with.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::ReportedOnly => "reported_only",
        }
    }
}

/// An observation word no run has.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown observation {0:?}; expected managed or reported_only")]
pub struct ObservationParseError(String);

impl std::str::FromStr for Observation {
    type Err = ObservationParseError;

    fn from_str(word: &str) -> Result<Self, Self::Err> {
        [Self::Managed, Self::ReportedOnly]
            .into_iter()
            .find(|mode| mode.word() == word)
            .ok_or_else(|| ObservationParseError(word.into()))
    }
}

/// An exact, immutable reference to the source a run executes against.
///
/// A branch or path is mutable, so it is not a source reference: name the commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunSource {
    /// A commit of a repository asset the work requires.
    GitCommit {
        /// Repository asset the task's contract requires.
        asset: AssetId,
        /// Full lowercase hexadecimal object name, 40 (SHA-1) or 64 (SHA-256) digits.
        commit: String,
    },
    /// Git commit evidence attached to this task, as `attach-git-head` records it: a `GitCommit`
    /// artifact whose `commit` and `asset_id` metadata and `git:asset:ASSET@COMMIT` URI name an
    /// exact commit of a repository the task requires. Evidence that names only a branch, or a
    /// different asset than its metadata, is not an exact source and is refused.
    Artifact {
        /// An artifact attached to the task.
        artifact: ArtifactId,
    },
}

/// The exact, immutable source content a run's source references resolved to when it started.
///
/// The engine fills this from the plan, so the record holds the commit itself and not only a
/// pointer to evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactSource {
    /// Repository asset the commit belongs to; a repository the task's contract requires.
    pub asset: AssetId,
    /// Full lowercase hexadecimal object name, 40 or 64 digits.
    pub commit: String,
    /// The attached evidence artifact that attested the commit, when the source named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactId>,
}

/// What a caller asks to record when a run starts; the application adds what it observes.
///
/// Two requests with equal fields are one request, which is how a retry is told from a conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunStart {
    /// Run identity and idempotency key, a version 7 UUID.
    pub id: RunId,
    /// The task being executed.
    pub work: WorkItemId,
    /// The principal doing the work; the task's owner when the run starts.
    pub executor: ActorId,
    /// Run that spawned this one, such as a parent agent's run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<RunId>,
    /// Provider session, absent for an executor with none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<RunSession>,
    /// How completely the executor is observed.
    pub observation: Observation,
    /// Exact source references; empty when none are known.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<RunSource>,
    /// When the executor says the run began. It is a claim, never used to judge freshness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<DateTime<Utc>>,
}

/// The contract a run executes against, as observed at its start.
///
/// This is typed immutable content: later reviewed plan changes edit the plan, not this record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractObservation {
    /// Workspace the contract was read from.
    pub workspace_id: WorkspaceId,
    /// Writable history the revision belongs to.
    pub lineage_id: LineageId,
    /// Plan revision the contract was read at, meaningful within the lineage.
    pub revision: u64,
    /// Human key of the work at that revision.
    pub work_key: Key,
    /// Submissions the work already had, so a rework run is told from the first attempt.
    pub prior_submissions: u32,
    /// The authored contract exactly as it stood at that revision.
    pub contract: WorkContract,
}

/// A run as recorded when it started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    /// Run identity.
    pub id: RunId,
    /// The task being executed.
    pub work: WorkItemId,
    /// The principal doing the work.
    pub executor: ActorId,
    /// The principal that recorded the run, the executor itself or a service acting for it.
    pub recorded_by: ActorId,
    /// Run that spawned this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<RunId>,
    /// Provider session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<RunSession>,
    /// How completely the executor is observed.
    pub observation: Observation,
    /// Exact source references as requested.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<RunSource>,
    /// The exact commits those references resolved to, captured immutably at the start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exact_sources: Vec<ExactSource>,
    /// The contract observed at the start.
    pub contract: ContractObservation,
    /// When the executor says the run began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_started_at: Option<DateTime<Utc>>,
    /// When DPM recorded the start; this is the receipt time, from DPM's own clock.
    pub started_at: DateTime<Utc>,
}

impl RunRecord {
    /// The request fields this record answers, for comparing a resent start with the recorded one.
    #[must_use]
    pub fn request(&self) -> RunStart {
        RunStart {
            id: self.id,
            work: self.work,
            executor: self.executor.clone(),
            parent: self.parent,
            session: self.session.clone(),
            observation: self.observation,
            sources: self.sources.clone(),
            observed_at: self.observed_started_at,
        }
    }
}

/// An explicit association of a committed project operation with the run that performed it.
///
/// Linking never changes the operation: the semantic log is unaware of runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunLink {
    /// The run that performed the operation.
    pub run: RunId,
    /// The committed operation.
    pub operation: OperationId,
    /// The task the operation targeted.
    pub work: WorkItemId,
    /// The principal that recorded the association.
    pub linked_by: ActorId,
    /// When DPM recorded the association.
    pub linked_at: DateTime<Utc>,
}
