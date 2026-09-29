//! Consistent backup, restore into a new location, and read-only integrity verification.
//!
//! Copying the database file of a live WAL store can miss committed pages still in `-wal` or tear
//! a page mid-write, and JSON export drops the operation history. Backups therefore use SQLite's
//! online backup API, which copies every page of one read snapshot while writers continue.
//! A backup is sealed as an archive of its source's lineage; a restore is sealed as a new lineage.

use super::{
    SqliteStore,
    integrity::{check_history_blocking, integrity_check_blocking},
    lineage::{self, Seal},
    load_blocking,
    replay::replay_blocking,
    snapshot::genesis_blocking,
};
use crate::{
    StoreError,
    error::database_error,
    schema::{self, Layout},
};
use dpm_model::{LineageId, WorkspaceId};
use files::{
    ReadMode, canonical_blocking, create_target_blocking, discard_blocking, fingerprint_blocking,
    open_read_only_blocking, read_mode_blocking, target_path_blocking,
};
use rusqlite::{
    Connection,
    backup::{Backup, StepResult},
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) mod files;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const BUSY_RETRY_DELAY: Duration = Duration::from_millis(100);
const BUSY_RETRIES: u32 = 50;

/// Outcome of a full read-only verification of one store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IntegrityReport {
    /// Verified database file as a canonical absolute path.
    pub path: PathBuf,
    /// Version stored in the header.
    pub schema_version: i64,
    /// Workspace identity a device binding or project locator must match.
    pub workspace_id: WorkspaceId,
    /// Workspace display name.
    pub workspace_name: String,
    /// Writable history the file continues; a restore reports a new one.
    pub lineage_id: LineageId,
    /// Whether the file is a backup archive that only a restore makes writable again.
    pub archived: bool,
    /// Snapshot revision, equal to the last operation's resulting revision.
    pub revision: u64,
    /// Number of committed operations in the history.
    pub operation_count: u64,
    /// Revision of the genesis plan recorded at initialization, which the history starts from.
    pub genesis_revision: u64,
}

impl SqliteStore {
    /// Copy the committed snapshot, full operation history and schema version into a new file,
    /// then verify the copy; an existing file is never overwritten.
    pub fn backup_blocking(&self, to: &Path) -> Result<IntegrityReport, StoreError> {
        let to = target_path_blocking(to)?;
        create_target_blocking(&to)?;
        // A backup is one self-contained file, so it carries no WAL side files.
        let result = copy_blocking(&self.connection, &to, "delete", Seal::Archive)
            .and_then(|()| verify_store_blocking(&to));
        discard_on_error_blocking(&to, result)
    }
}

/// Verify a backup, copy it into a new writable store file in WAL mode with a new lineage, and
/// verify the result; the source keeps its own lineage.
///
/// Live state is never overwritten: the target must not exist, and a failed restore removes it.
pub fn restore_store_blocking(from: &Path, to: &Path) -> Result<IntegrityReport, StoreError> {
    let from = canonical_blocking(from)?;
    let to = target_path_blocking(to)?;
    if !verify_store_blocking(&from)?.archived {
        return Err(crate::LineageError::NotAnArchive { path: from }.into());
    }
    let mode = read_mode_blocking(&from)?;
    let before = fingerprint_blocking(&from)?;
    let source = open_read_only_blocking(&from, mode)?;
    create_target_blocking(&to)?;
    let result = copy_blocking(&source, &to, "wal", Seal::Fork(LineageId::new())).and_then(|()| {
        // An immutable read cannot see a writer that started meanwhile, so the copy is only
        // trusted if the source provably did not change.
        if mode == ReadMode::Immutable && fingerprint_blocking(&from)? != before {
            return Err(StoreError::BackupIncomplete {
                path: to.clone(),
                state: format!("{} changed during the restore", from.display()),
            });
        }
        verify_store_blocking(&to)
    });
    discard_on_error_blocking(&to, result)
}

/// Check pages, exact layout, snapshot and history of a store in one read transaction, then replay
/// the history from the genesis plan and compare it with the snapshot, without writing to the store
/// or creating any file next to it. A divergence is reported, never repaired.
pub fn verify_store_blocking(path: &Path) -> Result<IntegrityReport, StoreError> {
    let path = canonical_blocking(path)?;
    if read_mode_blocking(&path)? == ReadMode::Immutable {
        let before = fingerprint_blocking(&path)?;
        let result = verify_with_blocking(&path, ReadMode::Immutable);
        // A writer that started meanwhile may have changed pages under the unlocked read.
        if fingerprint_blocking(&path)? == before {
            return result;
        }
    }
    verify_with_blocking(&path, ReadMode::Shared)
}

fn verify_with_blocking(path: &Path, mode: ReadMode) -> Result<IntegrityReport, StoreError> {
    let mut connection = open_read_only_blocking(path, mode)?;
    // One read transaction keeps a concurrent writer from separating snapshot and history.
    let transaction = connection
        .transaction()
        .map_err(database_error(path, "begin verification"))?;
    let schema_version = schema::stored_version_blocking(&transaction, path)?;
    let layout = schema::check_blocking(&transaction, path)?;
    if layout == Layout::Empty {
        return Err(StoreError::UnrecognizedSchema {
            path: path.to_path_buf(),
            detail: "no DPM tables".into(),
        });
    }
    integrity_check_blocking(&transaction, path)?;
    let plan = load_blocking(&transaction, path)?
        .ok_or_else(|| StoreError::NotInitialized(path.to_path_buf()))?;
    let genesis =
        genesis_blocking(&transaction, path)?.ok_or_else(|| StoreError::MissingGenesis {
            path: path.to_path_buf(),
        })?;
    let genesis_revision = genesis.revision;
    let store_lineage = lineage::read_blocking(&transaction, path)?;
    let history = check_history_blocking(
        &transaction,
        path,
        genesis.workspace.id,
        (genesis_revision, plan.revision),
    )?;
    replay_blocking(&transaction, path, genesis, &plan)?;
    Ok(IntegrityReport {
        path: path.to_path_buf(),
        schema_version,
        workspace_id: plan.workspace.id,
        workspace_name: plan.workspace.name,
        lineage_id: store_lineage.lineage_id,
        archived: store_lineage.archived,
        revision: plan.revision,
        operation_count: history.count,
        genesis_revision,
    })
}

fn copy_blocking(
    source: &Connection,
    to: &Path,
    journal_mode: &str,
    seal: Seal,
) -> Result<(), StoreError> {
    let mut target = Connection::open(to).map_err(database_error(to, "open backup target"))?;
    target
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(database_error(to, "set busy timeout"))?;
    {
        let backup =
            Backup::new(source, &mut target).map_err(database_error(to, "start backup"))?;
        let mut retries = 0;
        loop {
            // Copying all pages in one step reads a single snapshot of the source.
            match backup.step(-1).map_err(database_error(to, "copy pages"))? {
                StepResult::Done => break,
                StepResult::Busy | StepResult::Locked if retries < BUSY_RETRIES => {
                    retries += 1;
                    std::thread::sleep(BUSY_RETRY_DELAY);
                }
                state => {
                    return Err(StoreError::BackupIncomplete {
                        path: to.to_path_buf(),
                        state: format!("{state:?}"),
                    });
                }
            }
        }
    }
    // Sealed before the copy is verified or closed, so no unsealed copy is ever reported.
    lineage::seal_blocking(&target, to, seal)?;
    let mode: String = target
        .query_row(
            &format!("PRAGMA journal_mode = {journal_mode}"),
            [],
            |row| row.get(0),
        )
        .map_err(database_error(to, "set journal mode"))?;
    if !mode.eq_ignore_ascii_case(journal_mode) {
        return Err(StoreError::BackupIncomplete {
            path: to.to_path_buf(),
            state: format!("journal mode {mode} instead of {journal_mode}"),
        });
    }
    target
        .close()
        .map_err(|(_, source)| database_error(to, "close backup target")(source))
}

fn discard_on_error_blocking(
    to: &Path,
    result: Result<IntegrityReport, StoreError>,
) -> Result<IntegrityReport, StoreError> {
    if result.is_err() {
        discard_blocking(to);
    }
    result
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
