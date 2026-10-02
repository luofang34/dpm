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
    runs::{self, RunStore, RunStoreReport},
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
    /// The run store beside the project store, verified with it; absent when none was found
    /// beside it, which says nothing about whether runs were ever recorded. A project store file
    /// alone never contains runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runs: Option<RunStoreReport>,
}

impl SqliteStore {
    /// Copy the committed snapshot, full operation history and schema version into a new file,
    /// then verify the copy; an existing file is never overwritten.
    ///
    /// When the store has a run store beside it, that is copied too, to `<backup>.runs`, and the
    /// report says so; the project backup file alone never contains runs.
    pub fn backup_blocking(&self, to: &Path) -> Result<IntegrityReport, StoreError> {
        let to = target_path_blocking(to)?;
        let runs_to = RunStore::sidecar_path(&to);
        refuse_existing_blocking(&runs_to)?;
        let runs_from = RunStore::sidecar_path(&self.path);
        let has_runs = nonempty_blocking(&runs_from)?;
        create_target_blocking(&to)?;
        if has_runs {
            runs::recovery::create_target(&runs_to).inspect_err(|_| discard_blocking(&to))?;
        }
        let result = self
            .copy_runs_blocking(&runs_from, &runs_to, has_runs)
            .and_then(|()| {
                // A backup is one self-contained file, so it carries no WAL side files.
                copy_blocking(&self.connection, &to, "delete", |target| {
                    lineage::seal_blocking(target, &to, Seal::Archive)
                })
            })
            .and_then(|()| verify_store_blocking(&to));
        discard_all_on_error_blocking(&to, has_runs, result)
    }

    /// Copy the run store beside this store, if there is one, before the project store is copied.
    ///
    /// The two copies cannot share one snapshot. Taking the run store's first keeps the pair
    /// consistent whatever commits in between: a run store only links operations the project
    /// store had already committed, so a later project copy holds every operation the earlier run
    /// copy names, and only unlinked later operations differ. The reverse order could attribute an
    /// operation the project copy lacks, which verification refuses.
    fn copy_runs_blocking(&self, from: &Path, to: &Path, has_runs: bool) -> Result<(), StoreError> {
        if !has_runs {
            return Ok(());
        }
        let workspace = self
            .load_blocking()?
            .ok_or_else(|| StoreError::NotInitialized(self.path.clone()))?
            .workspace
            .id;
        RunStore::open_existing_blocking(from, workspace)
            .map_err(run_store_error(from))?
            .ok_or_else(|| missing_runs(from))?
            .backup_to_blocking(to)
            .map_err(run_store_error(to))
            .map(drop)
    }
}

/// Verify a backup, copy it into a new writable store file in WAL mode with a new lineage, and
/// verify the result; the source keeps its own lineage.
///
/// Live state is never overwritten: the target must not exist, and a failed restore removes it.
pub fn restore_store_blocking(from: &Path, to: &Path) -> Result<IntegrityReport, StoreError> {
    let from = canonical_blocking(from)?;
    let to = target_path_blocking(to)?;
    let source_report = verify_store_blocking(&from)?;
    if !source_report.archived {
        return Err(crate::LineageError::NotAnArchive { path: from }.into());
    }
    let runs_from = RunStore::sidecar_path(&from);
    let runs_to = RunStore::sidecar_path(&to);
    refuse_existing_blocking(&runs_to)?;
    let has_runs = nonempty_blocking(&runs_from)?;
    let mode = read_mode_blocking(&from)?;
    let before = fingerprint_blocking(&from)?;
    let source = open_read_only_blocking(&from, mode)?;
    create_target_blocking(&to)?;
    if has_runs {
        runs::recovery::create_target(&runs_to).inspect_err(|_| discard_blocking(&to))?;
    }
    let fork = LineageId::new();
    let result = copy_blocking(&source, &to, "wal", |target| {
        lineage::seal_blocking(target, &to, Seal::Fork(fork))
    })
    .and_then(|()| {
        // An immutable read cannot see a writer that started meanwhile, so the copy is only
        // trusted if the source provably did not change.
        if mode == ReadMode::Immutable && fingerprint_blocking(&from)? != before {
            return Err(StoreError::BackupIncomplete {
                path: to.clone(),
                state: format!("{} changed during the restore", from.display()),
            });
        }
        Ok(())
    })
    .and_then(|()| {
        if has_runs {
            runs::recovery::restore_blocking(
                &runs_from,
                &runs_to,
                source_report.workspace_id,
                fork,
                source_report.revision,
            )
            .map_err(run_store_error(&runs_to))?;
        }
        verify_store_blocking(&to)
    });
    discard_all_on_error_blocking(&to, has_runs, result)
}

/// Check pages, exact layout, snapshot and history of a store in one read transaction, then replay
/// the history from the genesis plan and compare it with the snapshot, without writing to the store
/// or creating any file next to it. A divergence is reported, never repaired.
///
/// A run store beside the file is verified with it, and the two must be a consistent pair: the run
/// store bound to this workspace and lineage, every run observing a revision this history holds,
/// and every operation link naming an operation in this history that passes the link rules. The
/// run store is read first and the project store after it, so a live pair does not fail because an
/// operation and its link arrived in between; a run store copied later than its project store does.
pub fn verify_store_blocking(path: &Path) -> Result<IntegrityReport, StoreError> {
    let path = canonical_blocking(path)?;
    let runs_path = RunStore::sidecar_path(&path);
    if !nonempty_blocking(&runs_path)? {
        return verify_project_blocking(&path, None);
    }
    let (runs, pair) = runs::recovery::verify_with_facts_blocking(&runs_path)
        .map_err(run_store_error(&runs_path))?;
    let report = verify_project_blocking(&path, Some(&pair))?;
    Ok(IntegrityReport {
        runs: Some(runs),
        ..report
    })
}

/// The project store, with its run store pair when one is given: pages, layout, snapshot and
/// history, then the replay.
fn verify_project_blocking(
    path: &Path,
    pair: Option<&runs::pairing::PairFacts>,
) -> Result<IntegrityReport, StoreError> {
    let path = canonical_blocking(path)?;
    if read_mode_blocking(&path)? == ReadMode::Immutable {
        let before = fingerprint_blocking(&path)?;
        let result = verify_with_blocking(&path, ReadMode::Immutable, pair);
        // A writer that started meanwhile may have changed pages under the unlocked read.
        if fingerprint_blocking(&path)? == before {
            return result;
        }
    }
    verify_with_blocking(&path, ReadMode::Shared, pair)
}

fn verify_with_blocking(
    path: &Path,
    mode: ReadMode,
    pair: Option<&runs::pairing::PairFacts>,
) -> Result<IntegrityReport, StoreError> {
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
    if let Some(pair) = pair {
        let identity = runs::pairing::ProjectIdentity {
            workspace: plan.workspace.id,
            lineage: store_lineage.lineage_id,
            snapshot_revision: plan.revision,
            genesis_revision,
        };
        runs::pairing::cross_check_blocking(&transaction, path, &identity, pair)
            .map_err(run_store_error(&RunStore::sidecar_path(path)))?;
    }
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
        runs: None,
    })
}

/// Copy every page of `source` into the new file `to`, let `seal` mark the copy before it is
/// closed, and set its journal mode.
pub(in crate::sqlite) fn copy_blocking(
    source: &Connection,
    to: &Path,
    journal_mode: &str,
    seal: impl FnOnce(&Connection) -> Result<(), StoreError>,
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
    seal(&target)?;
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

/// Remove the targets this command created, the run store's only if it was one of them.
fn discard_all_on_error_blocking(
    to: &Path,
    with_runs: bool,
    result: Result<IntegrityReport, StoreError>,
) -> Result<IntegrityReport, StoreError> {
    if result.is_err() {
        discard_blocking(to);
        if with_runs {
            runs::recovery::discard(&RunStore::sidecar_path(to));
        }
    }
    result
}

/// Whether a run store file exists beside a project store; an empty file left by an interrupted
/// creation holds no run.
fn nonempty_blocking(path: &Path) -> Result<bool, StoreError> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len() > 0),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(StoreError::Io {
            action: "inspect",
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// A run store beside a new target would be adopted by it, so a target never starts beside one.
fn refuse_existing_blocking(path: &Path) -> Result<(), StoreError> {
    let exists = path.try_exists().map_err(|source| StoreError::Io {
        action: "inspect",
        path: path.to_path_buf(),
        source,
    })?;
    if exists {
        return Err(StoreError::TargetExists {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// A failure of the run store beside a project store. Database, layout and I/O failures keep their
/// own typed errors; a finding about the run store's contents is reported as an integrity failure
/// of that file, which names it, so the project store itself is never blamed.
fn run_store_error(path: &Path) -> impl FnOnce(crate::RunStoreError) -> StoreError {
    let path = path.to_path_buf();
    move |source| match source {
        crate::RunStoreError::Store(error) => error,
        other => StoreError::IntegrityCheckFailed {
            path,
            problems: vec![other.to_string()],
        },
    }
}

fn missing_runs(path: &Path) -> StoreError {
    run_store_error(path)(crate::RunStoreError::Corrupt {
        path: path.to_path_buf(),
        detail: "the run store disappeared while it was being backed up".into(),
    })
}

#[cfg(test)]
mod tests;
