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
    /// The store was written by a binary with a different, unsupported layout version.
    #[error(
        "store at {path} has schema version {found}, but this dpm supports up to {supported}; \
         nothing was written: open it with the dpm release that wrote it, or back it up with that \
         release before upgrading"
    )]
    UnsupportedSchemaVersion {
        /// Database carrying the version.
        path: PathBuf,
        /// Version recorded in the store header.
        found: i64,
        /// Newest version this binary understands.
        supported: i64,
    },
    /// The file is SQLite but its tables are not a DPM store layout.
    #[error("{path} is not a DPM store: {detail}")]
    UnrecognizedSchema {
        /// Inspected database.
        path: PathBuf,
        /// First layout difference found.
        detail: String,
    },
    /// Backup and restore only ever create new files.
    #[error("{path} already exists; choose a new path, existing files are never overwritten")]
    TargetExists {
        /// Existing file or SQLite side file at the target location.
        path: PathBuf,
    },
    /// A filesystem operation outside SQLite failed.
    #[error("{action} {path}: {source}")]
    Io {
        /// Failed filesystem action.
        action: &'static str,
        /// Affected location.
        path: PathBuf,
        /// Original I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The online backup did not copy every page in one step.
    #[error("backup into {path} did not complete: {state}")]
    BackupIncomplete {
        /// Destination being written.
        path: PathBuf,
        /// SQLite step result that ended the copy.
        state: String,
    },
    /// SQLite's page-level integrity check reported damage.
    #[error("integrity check failed for {path}: {}", problems.join("; "))]
    IntegrityCheckFailed {
        /// Damaged database.
        path: PathBuf,
        /// First reported problems.
        problems: Vec<String>,
    },
    /// Operation revisions do not form one consecutive chain.
    #[error(
        "history of {path} breaks at sequence {sequence}: expected revision {expected}, found {found}"
    )]
    HistoryDiscontinuity {
        /// Inspected database.
        path: PathBuf,
        /// Local append sequence of the offending operation.
        sequence: u64,
        /// Revision the chain requires.
        expected: u64,
        /// Revision the operation records.
        found: u64,
    },
    /// The snapshot is not the state produced by the last recorded operation.
    #[error(
        "snapshot of {path} is at revision {snapshot}, but the last operation produced {last_operation}"
    )]
    SnapshotRevisionMismatch {
        /// Inspected database.
        path: PathBuf,
        /// Revision of the stored snapshot.
        snapshot: u64,
        /// Resulting revision of the last operation.
        last_operation: u64,
    },
    /// A stored JSON field no longer decodes.
    #[error("corrupt {record} {field} in {path}: {source}")]
    CorruptJson {
        /// Database containing the record.
        path: PathBuf,
        /// Snapshot or operation holding the field.
        record: StoredRecord,
        /// Column holding the damaged JSON.
        field: &'static str,
        /// Decoding failure.
        #[source]
        source: serde_json::Error,
    },
    /// A stored column holds a value of the wrong type.
    #[error("corrupt {record} {field} in {path}: {source}")]
    CorruptColumn {
        /// Database containing the record.
        path: PathBuf,
        /// Snapshot or operation holding the field.
        record: StoredRecord,
        /// Damaged column.
        field: &'static str,
        /// Column decoding failure.
        #[source]
        source: rusqlite::Error,
    },
    /// Local append sequences skip a value, so an operation row was removed.
    #[error("history of {path} skips from sequence {previous} to {sequence}")]
    SequenceGap {
        /// Inspected database.
        path: PathBuf,
        /// Sequence of the preceding operation.
        previous: u64,
        /// Next sequence found.
        sequence: u64,
    },
    /// The store records where its history starts, but no operation leads to the snapshot.
    #[error(
        "history of {path} is empty, but the snapshot moved from origin revision {origin} to {snapshot}"
    )]
    MissingHistory {
        /// Inspected database.
        path: PathBuf,
        /// Revision the history must start from.
        origin: u64,
        /// Revision of the stored snapshot.
        snapshot: u64,
    },
    /// A store in the current layout has a snapshot but no recorded history origin.
    #[error("{path} has a snapshot but no recorded history origin")]
    MissingOrigin {
        /// Inspected database.
        path: PathBuf,
    },
    /// Backup and restore targets must be a plain database file name in an existing directory.
    #[error("cannot write a store to {path}: {reason}")]
    InvalidTarget {
        /// Requested destination.
        path: PathBuf,
        /// Why the destination is unusable.
        reason: &'static str,
    },
}

/// Which stored record a corruption report refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredRecord {
    /// The authoritative plan snapshot.
    Snapshot,
    /// One operation, addressed by its local append sequence.
    Operation {
        /// Local append sequence.
        sequence: u64,
    },
    /// The recorded revision the operation history starts from.
    Origin,
}

impl std::fmt::Display for StoredRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Snapshot => formatter.write_str("snapshot"),
            Self::Operation { sequence } => write!(formatter, "operation sequence {sequence}"),
            Self::Origin => formatter.write_str("history origin"),
        }
    }
}

impl StoreError {
    /// Whether the stored data itself is damaged or inconsistent, as opposed to a refused request.
    pub fn is_corruption(&self) -> bool {
        match self {
            Self::IntegrityCheckFailed { .. }
            | Self::HistoryDiscontinuity { .. }
            | Self::SnapshotRevisionMismatch { .. }
            | Self::CorruptSnapshot { .. }
            | Self::CorruptJson { .. }
            | Self::CorruptColumn { .. }
            | Self::SequenceGap { .. }
            | Self::MissingHistory { .. }
            | Self::MissingOrigin { .. }
            | Self::UnrecognizedSchema { .. } => true,
            Self::Database { source, .. } => matches!(
                source.sqlite_error_code(),
                Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
            ),
            _ => false,
        }
    }
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
