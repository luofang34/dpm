//! Why a run store request was refused or could not be completed safely.

use crate::StoreError;
use dpm_model::{
    ActivityEntry, LifecycleEntry, LineageId, OperationId, RunId, RunRecord, WorkspaceId,
};
use std::path::PathBuf;
use thiserror::Error;

/// A run store operation that was refused or could not be completed.
///
/// Every refusal leaves the store as it was: writes are one transaction, and a file that cannot be
/// trusted is never opened for writing.
#[derive(Debug, Error)]
pub enum RunStoreError {
    /// The database, its layout, its lineage binding or a stored record failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The engine refuses the request: malformed, unauthorized or an illegal transition.
    #[error(transparent)]
    Run(#[from] dpm_engine::RunError),
    /// No run is recorded under this identity.
    #[error("run {0} is not recorded")]
    UnknownRun(RunId),
    /// The named parent run is not recorded.
    #[error("parent run {0} is not recorded")]
    UnknownParent(RunId),
    /// The run identity is already recorded with a different request.
    #[error("run {} is already recorded with different content; use a new run id for a new run", .recorded.id)]
    DuplicateRun {
        /// The run recorded under this identity.
        recorded: Box<RunRecord>,
    },
    /// The transition identity is already recorded with different content.
    #[error("transition {} is already recorded with different content; use a new event id", .recorded.event.id)]
    DuplicateEvent {
        /// The lifecycle fact recorded under this identity.
        recorded: Box<LifecycleEntry>,
    },
    /// The activity key is already recorded for the run with different content.
    #[error("activity {} of run {} is already recorded with different content", .recorded.record.source_sequence, .recorded.record.run)]
    DuplicateActivity {
        /// The activity record stored under this key.
        recorded: Box<ActivityEntry>,
    },
    /// The activity record's source sequence is at or below the run's high-water mark and the store
    /// no longer holds it: either a retry of a record retention already removed, or a record that
    /// arrived out of order. Nothing was written, and the run's receipt time did not move.
    #[error(
        "activity {source_sequence} of run {run} is expired: the run's accepted sequences already reach {high_water}, and this one is not retained; sequences must increase"
    )]
    ActivityExpired {
        /// The run.
        run: RunId,
        /// The refused record's source sequence.
        source_sequence: u64,
        /// The highest source sequence the run has accepted.
        high_water: u64,
    },
    /// The run was recorded under another lineage than the store's current one, such as before a
    /// restore. It stays a historical observation: nothing new is written to it.
    #[error(
        "run {run} was recorded under lineage {recorded}, but this store continues lineage {current}; it is a historical observation and takes no new facts, so start a new run"
    )]
    ForeignRun {
        /// The run.
        run: RunId,
        /// The lineage the run was recorded under.
        recorded: LineageId,
        /// The lineage the store continues.
        current: LineageId,
    },
    /// The operation is already linked to another run.
    #[error("operation {operation} is already linked to run {linked}")]
    LinkConflict {
        /// The operation.
        operation: OperationId,
        /// The run it is linked to.
        linked: RunId,
    },
    /// The run store belongs to another workspace than the project store it sits beside.
    #[error("run store {path} belongs to workspace {actual}, not {expected}; nothing was changed")]
    WorkspaceMismatch {
        /// Run store file.
        path: PathBuf,
        /// Workspace of the project store.
        expected: WorkspaceId,
        /// Workspace the run store is bound to.
        actual: WorkspaceId,
    },
    /// Stored run data is inconsistent, so the store cannot be trusted as it is.
    #[error("run store {path} is inconsistent: {detail}")]
    Corrupt {
        /// Inspected run store.
        path: PathBuf,
        /// First inconsistency found.
        detail: String,
    },
}

impl RunStoreError {
    /// Whether the stored data itself is damaged or inconsistent, as opposed to a refused request.
    #[must_use]
    pub fn is_corruption(&self) -> bool {
        match self {
            Self::Store(error) => error.is_corruption(),
            Self::Corrupt { .. } => true,
            _ => false,
        }
    }

    /// Whether another connection held the database lock past the busy timeout; nothing was
    /// written, and retrying later with the same identities is safe.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Store(error) if error.is_busy())
    }
}
