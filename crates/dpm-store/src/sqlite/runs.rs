//! The run store: a sidecar SQLite file holding agent runs, their durable lifecycle and bounded
//! activity, apart from the project store.
//!
//! Runs are observations, not project facts, so they never enter the semantic operation log or
//! change the plan revision. Keeping them in their own file leaves the project layout, replay and
//! writer lock untouched: activity writes contend only with other run writes. The file sits beside
//! the project store (`<store>.runs`), is bound to one workspace and one lineage when it is
//! created, and is refused, unchanged, if it is corrupt, of another version, or bound to another
//! workspace. Reading a store that was never created does not create it.

use crate::{
    LineageError, RunStoreError, StoreError, StoredRecord,
    error::database_error,
    schema::Layout,
    sqlite::{
        recovery::files,
        snapshot::{column, decode},
    },
};
use dpm_model::{LineageId, WorkspaceId};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

mod activity;
mod codec;
pub(super) mod pairing;
mod read;
pub(super) mod recovery;
mod schema;
mod write;

pub use recovery::RunStoreReport;
pub use schema::RUN_STORE_VERSION;

/// How long a run write waits for another run writer's lock before reporting the store busy.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Activity records kept by default; older ones are pruned and reported as a gap.
pub const RUN_ACTIVITY_LIMIT: u64 = 10_000;

/// The workspace and history a run store was created for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunBinding {
    /// Workspace whose project store this run store sits beside.
    pub workspace_id: WorkspaceId,
    /// Lineage of the project store when this run store was created or last restored. Writes are
    /// refused when the project store continues another lineage.
    pub lineage_id: LineageId,
    /// A backup that is read and verified but never written until restored.
    pub archived: bool,
}

/// The outcome of a write, and whether it only answered a repeat of an earlier one.
#[derive(Debug, Clone, PartialEq)]
pub struct Written<T> {
    /// What the store recorded.
    pub value: T,
    /// `true` when the identity was already recorded with the same content and nothing was
    /// written.
    pub replayed: bool,
}

/// A connection to one run store file.
pub struct RunStore {
    connection: Connection,
    path: PathBuf,
    activity_limit: u64,
}

impl RunStore {
    /// The run store that belongs beside the project store at `project`.
    #[must_use]
    pub fn sidecar_path(project: &Path) -> PathBuf {
        files::side_file(project, ".runs")
    }

    /// Open an existing run store for the project store of `workspace`, or `None` when none was
    /// ever created. Nothing is created or written; a store that is corrupt, of another version
    /// or bound to another workspace is refused before it is opened for writing.
    pub fn open_existing_blocking(
        path: &Path,
        workspace: WorkspaceId,
    ) -> Result<Option<Self>, RunStoreError> {
        let exists = path.try_exists().map_err(|source| StoreError::Io {
            action: "inspect",
            path: path.to_path_buf(),
            source,
        })?;
        if !exists {
            return Ok(None);
        }
        let probe = files::open_read_only_blocking(path, files::read_mode_blocking(path)?)?;
        match schema::check_blocking(&probe, path)? {
            Layout::Empty => return Ok(None),
            Layout::Current => check_workspace(&read_binding(&probe, path)?, workspace, path)?,
        }
        drop(probe);
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(database_error(path, "open run store"))?;
        Ok(Some(Self::prepare(connection, path.to_path_buf())?))
    }

    /// Open the run store beside a project store, creating it bound to `workspace` and `lineage`
    /// when it does not exist yet.
    pub fn open_or_create_blocking(
        path: &Path,
        workspace: WorkspaceId,
        lineage: LineageId,
    ) -> Result<Self, RunStoreError> {
        if let Some(store) = Self::open_existing_blocking(path, workspace)? {
            return Ok(store);
        }
        let connection =
            Connection::open(path).map_err(database_error(path, "create run store"))?;
        let mut store = Self::prepare(connection, path.to_path_buf())?;
        store.initialize_blocking(workspace, lineage)?;
        Ok(store)
    }

    /// A disposable run store for an in-memory project store.
    pub fn in_memory_blocking(
        workspace: WorkspaceId,
        lineage: LineageId,
    ) -> Result<Self, RunStoreError> {
        let path = PathBuf::from(":memory:");
        let connection =
            Connection::open_in_memory().map_err(database_error(&path, "open run store"))?;
        let mut store = Self::prepare(connection, path)?;
        store.initialize_blocking(workspace, lineage)?;
        Ok(store)
    }

    fn prepare(connection: Connection, path: PathBuf) -> Result<Self, StoreError> {
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(database_error(&path, "set busy timeout"))?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(database_error(&path, "configure run store"))?;
        Ok(Self {
            connection,
            path,
            activity_limit: RUN_ACTIVITY_LIMIT,
        })
    }

    /// Create the layout in an empty database, unless a concurrent creator already did.
    fn initialize_blocking(
        &mut self,
        workspace: WorkspaceId,
        lineage: LineageId,
    ) -> Result<(), RunStoreError> {
        // The journal mode cannot change inside a transaction.
        self.connection
            .execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(database_error(&self.path, "configure run store"))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error(&self.path, "begin run store creation"))?;
        if schema::check_blocking(&transaction, &self.path)? == Layout::Empty {
            schema::create_blocking(&transaction, &self.path)?;
            transaction
                .execute(
                    "INSERT INTO run_binding(singleton, workspace_id, lineage_id, archived) \
                     VALUES(1, ?1, ?2, 0)",
                    rusqlite::params![workspace.to_string(), lineage.to_string()],
                )
                .map_err(database_error(&self.path, "bind run store"))?;
            transaction
                .execute(
                    "INSERT INTO run_activity_retention(singleton, pruned_through) VALUES(1, 0)",
                    [],
                )
                .map_err(database_error(&self.path, "record retention"))?;
        }
        check_workspace(
            &read_binding(&transaction, &self.path)?,
            workspace,
            &self.path,
        )?;
        transaction
            .commit()
            .map_err(database_error(&self.path, "commit run store creation"))?;
        Ok(())
    }

    /// Keep at most `limit` activity records from now on; older ones are pruned by the next write.
    pub fn set_activity_limit(&mut self, limit: u64) {
        self.activity_limit = limit;
    }

    /// Location of the run store, or the in-memory label.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The workspace and history this run store is bound to.
    pub fn binding_blocking(&self) -> Result<RunBinding, RunStoreError> {
        Ok(read_binding(&self.connection, &self.path)?)
    }

    fn read_transaction(&self) -> Result<Transaction<'_>, StoreError> {
        self.connection
            .unchecked_transaction()
            .map_err(database_error(&self.path, "begin run query"))
    }
}

fn check_workspace(
    binding: &RunBinding,
    expected: WorkspaceId,
    path: &Path,
) -> Result<(), RunStoreError> {
    if binding.workspace_id == expected {
        return Ok(());
    }
    Err(RunStoreError::WorkspaceMismatch {
        path: path.to_path_buf(),
        expected,
        actual: binding.workspace_id,
    })
}

pub(super) fn read_binding(connection: &Connection, path: &Path) -> Result<RunBinding, StoreError> {
    let mut statement = connection
        .prepare("SELECT workspace_id, lineage_id, archived FROM run_binding WHERE singleton = 1")
        .map_err(database_error(path, "prepare run binding query"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "load run binding"))?;
    let Some(row) = rows
        .next()
        .map_err(database_error(path, "load run binding"))?
    else {
        return Err(StoreError::Lineage(LineageError::Missing {
            path: path.to_path_buf(),
        }));
    };
    let scalar = |index, field| -> Result<String, StoreError> {
        column::<String>(row, index, path, StoredRecord::Run, field)
            .map(|text| serde_json::Value::String(text).to_string())
    };
    Ok(RunBinding {
        workspace_id: decode(
            &scalar(0, "workspace_id")?,
            path,
            StoredRecord::Run,
            "workspace_id",
        )?,
        lineage_id: decode(
            &scalar(1, "lineage_id")?,
            path,
            StoredRecord::Run,
            "lineage_id",
        )?,
        archived: column(row, 2, path, StoredRecord::Run, "archived")?,
    })
}

#[cfg(test)]
mod tests;
