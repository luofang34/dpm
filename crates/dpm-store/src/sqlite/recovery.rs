//! Consistent backup, restore into a new location, and read-only integrity verification.
//!
//! Copying the database file of a live WAL store can miss committed pages still in `-wal` or tear
//! a page mid-write, and JSON export drops the operation history. Backups therefore use SQLite's
//! online backup API, which copies every page of one read snapshot while writers continue.

use super::{
    SqliteStore,
    history::{OPERATION_COLUMNS, read_entry},
    load_blocking,
};
use crate::{
    StoreError,
    error::database_error,
    schema::{self, Layout},
};
use dpm_model::WorkspaceId;
use rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use serde::Serialize;
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const BUSY_RETRY_DELAY: Duration = Duration::from_millis(100);
const BUSY_RETRIES: u32 = 50;
const PROBLEM_LIMIT: usize = 10;
const SIDE_FILE_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];

/// Outcome of a full read-only verification of one store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IntegrityReport {
    /// Verified database file.
    pub path: PathBuf,
    /// Version stored in the header; 0 is the unversioned baseline of version 1.
    pub schema_version: i64,
    /// Workspace identity a device binding or project locator must match.
    pub workspace_id: WorkspaceId,
    /// Workspace display name.
    pub workspace_name: String,
    /// Snapshot revision, equal to the last operation's resulting revision.
    pub revision: u64,
    /// Number of committed operations in the history.
    pub operation_count: u64,
    /// Revision the history starts from, absent for a store without operations.
    pub first_base_revision: Option<u64>,
}

impl SqliteStore {
    /// Copy the committed snapshot, full operation history and schema version into a new file,
    /// then verify the copy; an existing file is never overwritten.
    pub fn backup_blocking(&self, to: &Path) -> Result<IntegrityReport, StoreError> {
        create_target_blocking(to)?;
        // A backup is one self-contained file, so it carries no WAL side files.
        let result =
            copy_blocking(&self.connection, to, "delete").and_then(|()| verify_store_blocking(to));
        discard_on_error_blocking(to, result)
    }
}

/// Verify a backup, copy it into a new store file in WAL mode and verify the result.
///
/// Live state is never overwritten: the target must not exist, and a failed restore removes it.
pub fn restore_store_blocking(from: &Path, to: &Path) -> Result<IntegrityReport, StoreError> {
    verify_store_blocking(from)?;
    let source = open_read_only_blocking(from)?;
    create_target_blocking(to)?;
    let result = copy_blocking(&source, to, "wal").and_then(|()| verify_store_blocking(to));
    discard_on_error_blocking(to, result)
}

/// Check pages, layout, snapshot and history of a store in one read transaction, without writes.
pub fn verify_store_blocking(path: &Path) -> Result<IntegrityReport, StoreError> {
    let mut connection = open_read_only_blocking(path)?;
    // One read transaction keeps a concurrent writer from separating snapshot and history.
    let transaction = connection
        .transaction()
        .map_err(database_error(path, "begin verification"))?;
    let schema_version = schema::stored_version_blocking(&transaction, path)?;
    if schema::check_blocking(&transaction, path)? == Layout::Empty {
        return Err(StoreError::UnrecognizedSchema {
            path: path.to_path_buf(),
            detail: "no DPM tables".into(),
        });
    }
    integrity_check_blocking(&transaction, path)?;
    let plan = load_blocking(&transaction, path)?
        .ok_or_else(|| StoreError::NotInitialized(path.to_path_buf()))?;
    let (operation_count, first_base_revision) =
        check_history_blocking(&transaction, path, plan.revision)?;
    Ok(IntegrityReport {
        path: path.to_path_buf(),
        schema_version,
        workspace_id: plan.workspace.id,
        workspace_name: plan.workspace.name,
        revision: plan.revision,
        operation_count,
        first_base_revision,
    })
}

fn integrity_check_blocking(connection: &Connection, path: &Path) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare("PRAGMA integrity_check")
        .map_err(database_error(path, "prepare integrity check"))?;
    let problems = statement
        .query_map([], |row| row.get(0))
        .map_err(database_error(path, "run integrity check"))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(database_error(path, "read integrity check"))?;
    if problems == ["ok"] {
        return Ok(());
    }
    Err(StoreError::IntegrityCheckFailed {
        path: path.to_path_buf(),
        problems: problems.into_iter().take(PROBLEM_LIMIT).collect(),
    })
}

fn check_history_blocking(
    connection: &Connection,
    path: &Path,
    snapshot: u64,
) -> Result<(u64, Option<u64>), StoreError> {
    let mut statement = connection
        .prepare(&format!("{OPERATION_COLUMNS} ORDER BY sequence"))
        .map_err(database_error(path, "prepare history verification"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "query history"))?;
    let (mut count, mut first, mut previous) = (0_u64, None, None);
    while let Some(row) = rows.next().map_err(database_error(path, "read history"))? {
        let entry = read_entry(row, path)?;
        let operation = &entry.operation;
        let breaks = |expected, found| StoreError::HistoryDiscontinuity {
            path: path.to_path_buf(),
            sequence: entry.sequence,
            expected,
            found,
        };
        if let Some(expected) = previous
            && expected != operation.base_revision
        {
            return Err(breaks(expected, operation.base_revision));
        }
        let expected = operation.base_revision.wrapping_add(1);
        if operation.resulting_revision != expected {
            return Err(breaks(expected, operation.resulting_revision));
        }
        first.get_or_insert(operation.base_revision);
        previous = Some(operation.resulting_revision);
        count = count.wrapping_add(1);
    }
    if let Some(last_operation) = previous
        && last_operation != snapshot
    {
        return Err(StoreError::SnapshotRevisionMismatch {
            path: path.to_path_buf(),
            snapshot,
            last_operation,
        });
    }
    Ok((count, first))
}

fn copy_blocking(source: &Connection, to: &Path, journal_mode: &str) -> Result<(), StoreError> {
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

fn open_read_only_blocking(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(database_error(path, "open database read-only"))?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(database_error(path, "set busy timeout"))?;
    Ok(connection)
}

fn side_file(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn create_target_blocking(to: &Path) -> Result<(), StoreError> {
    // A stale side file would be read as part of the new database.
    for suffix in SIDE_FILE_SUFFIXES {
        let side = side_file(to, suffix);
        let exists = side.try_exists().map_err(|source| StoreError::Io {
            action: "inspect",
            path: side.clone(),
            source,
        })?;
        if exists {
            return Err(StoreError::TargetExists { path: side });
        }
    }
    // Exclusive creation closes the race between checking for and writing the target.
    match fs::OpenOptions::new().write(true).create_new(true).open(to) {
        Ok(_) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            Err(StoreError::TargetExists {
                path: to.to_path_buf(),
            })
        }
        Err(source) => Err(StoreError::Io {
            action: "create",
            path: to.to_path_buf(),
            source,
        }),
    }
}

fn discard_on_error_blocking(
    to: &Path,
    result: Result<IntegrityReport, StoreError>,
) -> Result<IntegrityReport, StoreError> {
    if result.is_err() {
        // The copy failure is the actionable error; a leftover file is reported as TargetExists
        // on the next attempt rather than masking that cause.
        for path in SIDE_FILE_SUFFIXES
            .iter()
            .map(|suffix| side_file(to, suffix))
            .chain([to.to_path_buf()])
        {
            fs::remove_file(path).ok();
        }
    }
    result
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
