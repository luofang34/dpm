use dpm_engine::EngineError;
use dpm_model::ValidationError;
use std::path::PathBuf;
use thiserror::Error;

/// A persistence operation that could not be completed safely.
#[derive(Debug, Error)]
pub enum StoreError {
    /// SQLite failed while accessing the named database.
    #[cfg(feature = "sqlite")]
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
    #[error("corrupt {record} at {path}: row revision {row}, JSON revision {json}")]
    CorruptSnapshot {
        /// Database containing the inconsistent plan.
        path: PathBuf,
        /// Snapshot or genesis plan.
        record: StoredRecord,
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
    /// The store was written in a retired layout whose operation log cannot be replayed.
    #[error(
        "store at {path} has schema version {found}, which this dpm no longer opens (it writes \
         version {current}); nothing was written. Back it up and keep it as the archive of its \
         history, run `dpm export` with the dpm release that wrote it, and import that export \
         into a new store with `dpm import <file>`"
    )]
    RetiredSchemaVersion {
        /// Database carrying the version.
        path: PathBuf,
        /// Version recorded in the store header; 0 for DPM tables without a header.
        found: i64,
        /// Version this binary writes.
        current: i64,
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
    #[cfg(feature = "sqlite")]
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
    /// No operation leads from the genesis plan to the snapshot.
    #[error(
        "history of {path} is empty, but the snapshot moved from genesis revision {origin} to {snapshot}"
    )]
    MissingHistory {
        /// Inspected database.
        path: PathBuf,
        /// Revision the history must start from.
        origin: u64,
        /// Revision of the stored snapshot.
        snapshot: u64,
    },
    /// A store has a snapshot but no genesis plan to replay its history from.
    #[error("{path} has a snapshot but no genesis plan")]
    MissingGenesis {
        /// Inspected database.
        path: PathBuf,
    },
    /// The engine refuses a recorded operation when the history is replayed from genesis.
    #[error("replaying the history of {path} fails at sequence {sequence}: {source}")]
    ReplayRefused {
        /// Inspected database.
        path: PathBuf,
        /// Local append sequence of the refused operation.
        sequence: u64,
        /// Engine refusal.
        #[source]
        source: EngineError,
    },
    /// Replaying genesis and every operation does not reproduce the stored snapshot.
    #[error(
        "replaying the history of {path} to revision {revision} does not reproduce the snapshot; \
         differing: {}", differing.join(", ")
    )]
    ReplayDiverged {
        /// Inspected database.
        path: PathBuf,
        /// Revision of the stored snapshot.
        revision: u64,
        /// Top-level plan fields whose replayed value differs from the snapshot.
        differing: Vec<String>,
    },
    /// The store's lineage is missing, archived, or not the one the caller observed.
    #[error(transparent)]
    Lineage(#[from] LineageError),
    /// An operation belongs to a different workspace than the store's genesis plan.
    #[error("operation sequence {sequence} in {path} belongs to workspace {found}, not {expected}")]
    ForeignOperation {
        /// Inspected database.
        path: PathBuf,
        /// Local append sequence of the offending operation.
        sequence: u64,
        /// Workspace the operation records.
        found: dpm_model::WorkspaceId,
        /// Workspace of the genesis plan.
        expected: dpm_model::WorkspaceId,
    },
    /// The operation identity is already recorded; the caller decides whether the resend matches.
    #[error("operation {} is already recorded at sequence {}", .recorded.operation.operation.id, .recorded.sequence)]
    DuplicateOperation {
        /// The recorded operation with this identity.
        recorded: Box<crate::HistoryEntry>,
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

/// Why a store's lineage does not permit a read or write.
#[derive(Debug, Error)]
pub enum LineageError {
    /// A store has no lineage row, so its writable history cannot be identified.
    #[error("{path} records no lineage")]
    Missing {
        /// Inspected database.
        path: PathBuf,
    },
    /// A restore forks a new writable lineage, so its source must be a sealed backup archive: an
    /// interrupted copy of an archive stays sealed, while one of a live store would be a writable
    /// file sharing that store's lineage.
    #[error(
        "{path} is a live store, not a backup archive; run `dpm backup` first and restore the backup"
    )]
    NotAnArchive {
        /// Live store named as the restore source.
        path: PathBuf,
    },
    /// A backup archive is read and verified, never written.
    #[error(
        "{path} is a backup archive; restore it into a new store with `dpm restore` to continue \
         writing"
    )]
    Archived {
        /// Archive the write targeted.
        path: PathBuf,
    },
    /// The store continues a different writable history than the caller observed.
    #[error(
        "lineage conflict: store continues lineage {actual}, operation expected {expected}; \
         reload before writing, histories are never merged"
    )]
    Mismatch {
        /// Lineage the caller observed.
        expected: dpm_model::LineageId,
        /// Lineage of the store.
        actual: dpm_model::LineageId,
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
    /// The plan the operation history replays from.
    Genesis,
    /// The lineage row of the file.
    Lineage,
}

impl std::fmt::Display for StoredRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Snapshot => formatter.write_str("snapshot"),
            Self::Operation { sequence } => write!(formatter, "operation sequence {sequence}"),
            Self::Genesis => formatter.write_str("genesis plan"),
            Self::Lineage => formatter.write_str("lineage"),
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
            | Self::SequenceGap { .. }
            | Self::MissingHistory { .. }
            | Self::MissingGenesis { .. }
            | Self::Lineage(LineageError::Missing { .. })
            | Self::ForeignOperation { .. }
            | Self::ReplayRefused { .. }
            | Self::ReplayDiverged { .. }
            | Self::UnrecognizedSchema { .. } => true,
            #[cfg(feature = "sqlite")]
            Self::CorruptColumn { .. } => true,
            #[cfg(feature = "sqlite")]
            Self::Database { source, .. } => matches!(
                source.sqlite_error_code(),
                Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
            ),
            _ => false,
        }
    }

    /// Whether another connection held the database lock past the busy timeout; retrying later
    /// may succeed, and nothing was written.
    pub fn is_busy(&self) -> bool {
        #[cfg(feature = "sqlite")]
        if let Self::Database { source, .. } = self {
            return matches!(
                source.sqlite_error_code(),
                Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
            );
        }
        false
    }
}

#[cfg(feature = "sqlite")]
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
