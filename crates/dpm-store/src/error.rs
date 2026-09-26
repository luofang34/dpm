use dpm_engine::EngineError;
use dpm_model::ValidationError;
use std::path::PathBuf;
use thiserror::Error;

/// A persistence operation that could not be completed safely.
#[derive(Debug, Error)]
pub enum StoreError {
    /// SQLite failed while accessing the named database.
    #[error("{action} in {path}: {source}")]
    Database {
        /// Database location or the in-memory label.
        path: PathBuf,
        /// Failed persistence action.
        action: &'static str,
        /// Original database error.
        #[source]
        source: rusqlite::Error,
    },
    /// Snapshot or operation serialization failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The supplied graph violates model invariants.
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// The supplied operation cannot produce a valid state transition.
    #[error(transparent)]
    Engine(#[from] EngineError),
    /// A caller attempted to replace existing history during initialization.
    #[error("workspace already exists at {0}")]
    AlreadyInitialized(PathBuf),
    /// Persistence requires an existing initial snapshot.
    #[error("workspace is not initialized at {0}")]
    NotInitialized(PathBuf),
    /// The caller's base revision is stale.
    #[error("revision conflict: store has {actual}, operation expected {expected}")]
    RevisionConflict {
        /// Revision required by the caller.
        expected: u64,
        /// Current stored revision.
        actual: u64,
    },
    /// Snapshot revisions do not describe one consecutive wrapping operation.
    #[error("invalid operation revision: base {base}, result {result}, plan {plan}")]
    InvalidOperationRevision {
        /// Revision before the operation.
        base: u64,
        /// Claimed resulting revision.
        result: u64,
        /// Supplied snapshot revision.
        plan: u64,
    },
    /// Snapshot contents differ from the actual result of the supplied command.
    #[error("snapshot does not match operation {0}")]
    SnapshotMismatch(dpm_model::OperationId),
    /// Stored revision metadata disagrees with the serialized plan.
    #[error("corrupt snapshot at {path}: row revision {row}, JSON revision {json}")]
    CorruptSnapshot {
        /// Database containing the inconsistent snapshot.
        path: PathBuf,
        /// Revision in the SQLite row.
        row: u64,
        /// Revision inside the plan JSON.
        json: u64,
    },
}

pub(crate) fn database_error(
    path: &std::path::Path,
    action: &'static str,
) -> impl FnOnce(rusqlite::Error) -> StoreError {
    let path = path.to_path_buf();
    move |source| StoreError::Database {
        path,
        action,
        source,
    }
}
