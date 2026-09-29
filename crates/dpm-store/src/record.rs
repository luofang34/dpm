//! Records the store returns to adapters, available whether or not a SQLite backend is built.

use crate::LineageError;
use dpm_engine::Operation;
use dpm_model::{LineageId, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A committed operation with the workspace and writable history it was recorded in; mutation
/// results and history entries return the same object, so a resent operation is answered with
/// exactly what the first attempt returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedOperation {
    /// Identity, actor, timestamp, revision precondition and semantic command.
    #[serde(flatten)]
    pub operation: Operation,
    /// Workspace the operation changed.
    pub workspace_id: WorkspaceId,
    /// Lineage of the store that committed it; a restored store keeps the lineages of the
    /// operations it copied and records later ones under its own.
    pub lineage_id: LineageId,
}

/// One immutable operation addressed by its local append order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Local database cursor, independent of wrapping domain revisions.
    pub sequence: u64,
    /// The recorded operation.
    pub operation: RecordedOperation,
}

/// A bounded operation page read with a consistent authoritative revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryPage {
    /// Revision visible in the same SQLite transaction.
    pub revision: u64,
    /// Lineage the store continues, within which the revision is meaningful; absent only for a
    /// page an adapter builds without a store.
    pub lineage_id: Option<LineageId>,
    /// Operations in ascending append order.
    pub entries: Vec<HistoryEntry>,
    /// Last returned sequence, suitable for the next request; unchanged for an empty page.
    pub next_after_sequence: u64,
}

/// The lineage row of one store file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreLineage {
    /// Writable history this file continues.
    pub lineage_id: LineageId,
    /// A backup archive, refused for writing until restored into a new store.
    pub archived: bool,
}

impl StoreLineage {
    /// Refuse a write to an archive, or to a lineage other than the one the caller observed.
    pub fn check_writable(
        &self,
        expected: Option<LineageId>,
        path: &Path,
    ) -> Result<(), LineageError> {
        if self.archived {
            return Err(LineageError::Archived {
                path: path.to_path_buf(),
            });
        }
        match expected {
            Some(expected) if expected != self.lineage_id => Err(LineageError::Mismatch {
                expected,
                actual: self.lineage_id,
            }),
            _ => Ok(()),
        }
    }
}

/// The authoritative revision of a store and the lineage it belongs to, read without decoding the
/// plan, so clients can poll it for changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreRevision {
    /// Revision of the committed snapshot.
    pub revision: u64,
    /// Lineage within which the revision is meaningful.
    pub lineage: StoreLineage,
}
