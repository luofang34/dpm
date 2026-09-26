use crate::{StoreError, error::database_error};
use dpm_engine::{Operation, apply_command};
use dpm_model::Plan;
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

mod snapshot;
use snapshot::{load_blocking, revision_to_sql};

/// A synchronous local database with validated snapshots and immutable operation history.
pub struct SqliteStore {
    connection: Connection,
    path: PathBuf,
}

impl SqliteStore {
    /// Create or open a database and initialize its schema without replacing workspace data.
    pub fn open_blocking(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let connection = Connection::open(&path).map_err(database_error(&path, "open database"))?;
        Self::prepare_blocking(connection, path, true)
    }

    /// Open an existing database without creating a new file or modifying its schema.
    pub fn open_existing_blocking(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(database_error(&path, "open existing database"))?;
        Self::prepare_blocking(connection, path, false)
    }

    /// Create an isolated in-memory database for ephemeral work and tests.
    pub fn in_memory_blocking() -> Result<Self, StoreError> {
        let path = PathBuf::from(":memory:");
        let connection =
            Connection::open_in_memory().map_err(database_error(&path, "open database"))?;
        Self::prepare_blocking(connection, path, true)
    }

    fn prepare_blocking(
        connection: Connection,
        path: PathBuf,
        migrate: bool,
    ) -> Result<Self, StoreError> {
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(database_error(&path, "set busy timeout"))?;
        let store = Self { connection, path };
        if migrate {
            store.migrate_blocking()?;
        }
        Ok(store)
    }

    fn migrate_blocking(&self) -> Result<(), StoreError> {
        self.connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS plan_state (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 revision INTEGER NOT NULL,
                 plan_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS operations (
                 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                 operation_id TEXT NOT NULL UNIQUE,
                 base_revision INTEGER NOT NULL,
                 resulting_revision INTEGER NOT NULL,
                 actor_json TEXT NOT NULL,
                 timestamp TEXT NOT NULL,
                 command_json TEXT NOT NULL
             );",
            )
            .map_err(database_error(&self.path, "initialize schema"))?;
        Ok(())
    }

    /// Read and validate the authoritative snapshot, if initialized.
    pub fn load_blocking(&self) -> Result<Option<Plan>, StoreError> {
        load_blocking(&self.connection, &self.path)
    }

    /// Initialize an empty database; an existing snapshot or operation history is never erased.
    pub fn initialize_blocking(&mut self, plan: &Plan) -> Result<(), StoreError> {
        plan.validate()?;
        let json = serde_json::to_string(plan)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error(&self.path, "begin initialization"))?;
        let occupied: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM plan_state) OR EXISTS(SELECT 1 FROM operations)",
                [],
                |row| row.get(0),
            )
            .map_err(database_error(&self.path, "check initialization"))?;
        if occupied {
            return Err(StoreError::AlreadyInitialized(self.path.clone()));
        }
        transaction
            .execute(
                "INSERT INTO plan_state(singleton, revision, plan_json) VALUES(1, ?1, ?2)",
                params![revision_to_sql(plan.revision), json],
            )
            .map_err(database_error(&self.path, "initialize snapshot"))?;
        transaction
            .commit()
            .map_err(database_error(&self.path, "commit initialization"))?;
        Ok(())
    }

    /// Atomically append one valid operation and the exact snapshot produced by that command.
    pub fn persist_blocking(
        &mut self,
        plan: &Plan,
        operation: &Operation,
    ) -> Result<(), StoreError> {
        if operation.base_revision.wrapping_add(1) != operation.resulting_revision
            || plan.revision != operation.resulting_revision
        {
            return Err(StoreError::InvalidOperationRevision {
                base: operation.base_revision,
                result: operation.resulting_revision,
                plan: plan.revision,
            });
        }
        plan.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error(&self.path, "begin operation"))?;
        let mut expected = load_blocking(&transaction, &self.path)?
            .ok_or_else(|| StoreError::NotInitialized(self.path.clone()))?;
        if expected.revision != operation.base_revision {
            return Err(StoreError::RevisionConflict {
                expected: operation.base_revision,
                actual: expected.revision,
            });
        }
        apply_command(
            &mut expected,
            operation.actor.clone(),
            operation.command.clone(),
            operation.timestamp,
        )?;
        if expected != *plan {
            return Err(StoreError::SnapshotMismatch(operation.id));
        }
        snapshot::write_operation_blocking(&transaction, &self.path, plan, operation)?;
        transaction
            .commit()
            .map_err(database_error(&self.path, "commit operation"))?;
        Ok(())
    }

    /// Count committed semantic operations.
    pub fn operation_count_blocking(&self) -> Result<u64, StoreError> {
        let count: u64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
            .map_err(database_error(&self.path, "count operations"))?;
        Ok(count)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
