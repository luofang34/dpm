use crate::{
    StoreError,
    error::database_error,
    schema::{self, Layout},
};
use dpm_engine::{Operation, apply_command};
use dpm_model::Plan;
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

mod history;
mod integrity;
mod recovery;
pub use history::{HistoryEntry, HistoryPage};
pub use recovery::{IntegrityReport, restore_store_blocking, verify_store_blocking};
mod snapshot;
use snapshot::{load_blocking, revision_to_sql};

/// A synchronous local database with validated snapshots and immutable operation history.
pub struct SqliteStore {
    connection: Connection,
    path: PathBuf,
}

impl SqliteStore {
    /// Create or open a database without writing to it; [`Self::initialize_blocking`] creates the
    /// schema.
    pub fn open_blocking(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let connection = Connection::open(&path).map_err(database_error(&path, "open database"))?;
        Self::prepare_blocking(connection, path)
    }

    /// Open an existing database without creating a new file or writing to it; a store written
    /// before the current schema version is upgraded by its next write.
    pub fn open_existing_blocking(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(database_error(&path, "open existing database"))?;
        Self::prepare_blocking(connection, path)
    }

    /// Create an isolated in-memory database for ephemeral work and tests.
    pub fn in_memory_blocking() -> Result<Self, StoreError> {
        let path = PathBuf::from(":memory:");
        let connection =
            Connection::open_in_memory().map_err(database_error(&path, "open database"))?;
        Self::prepare_blocking(connection, path)
    }

    fn prepare_blocking(connection: Connection, path: PathBuf) -> Result<Self, StoreError> {
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(database_error(&path, "set busy timeout"))?;
        // Opening writes nothing, so an unsupported or refused store is left byte-for-byte
        // untouched; schema creation and upgrades happen inside the first write transaction.
        schema::check_blocking(&connection, &path)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(database_error(&path, "configure database"))?;
        Ok(Self { connection, path })
    }

    /// Read and validate the authoritative snapshot, if initialized.
    pub fn load_blocking(&self) -> Result<Option<Plan>, StoreError> {
        if schema::check_blocking(&self.connection, &self.path)? == Layout::Empty {
            return Ok(None);
        }
        load_blocking(&self.connection, &self.path)
    }

    /// Initialize an empty database; an existing snapshot or operation history is never erased,
    /// and a refused initialization writes nothing.
    pub fn initialize_blocking(&mut self, plan: &Plan) -> Result<(), StoreError> {
        plan.validate()?;
        let json = serde_json::to_string(plan)?;
        if occupied_blocking(&self.connection, &self.path)? {
            return Err(StoreError::AlreadyInitialized(self.path.clone()));
        }
        // The journal mode cannot change inside a transaction.
        self.connection
            .execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(database_error(&self.path, "configure database"))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error(&self.path, "begin initialization"))?;
        // A concurrent creator may have committed between the unlocked check and this lock.
        prepare_write_blocking(&transaction, &self.path)?;
        if occupied_blocking(&transaction, &self.path)? {
            return Err(StoreError::AlreadyInitialized(self.path.clone()));
        }
        transaction
            .execute(
                "INSERT INTO plan_state(singleton, revision, plan_json) VALUES(1, ?1, ?2)",
                params![revision_to_sql(plan.revision), json],
            )
            .map_err(database_error(&self.path, "initialize snapshot"))?;
        snapshot::write_origin_blocking(&transaction, &self.path, plan.revision)?;
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
        if prepare_write_blocking(&transaction, &self.path)? == Layout::Empty {
            return Err(StoreError::NotInitialized(self.path.clone()));
        }
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
            operation.id,
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
        if schema::check_blocking(&self.connection, &self.path)? == Layout::Empty {
            return Ok(0);
        }
        let count: u64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
            .map_err(database_error(&self.path, "count operations"))?;
        Ok(count)
    }
}

fn occupied_blocking(connection: &Connection, path: &Path) -> Result<bool, StoreError> {
    if schema::check_blocking(connection, path)? == Layout::Empty {
        return Ok(false);
    }
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM plan_state) OR EXISTS(SELECT 1 FROM operations)",
            [],
            |row| row.get(0),
        )
        .map_err(database_error(path, "check initialization"))
}

/// Bring the schema to the current layout inside a caller-owned write transaction and return the
/// layout found before that.
///
/// The exact layout is re-checked under the write lock, so a schema object added by another
/// process after open never runs inside this write. An originless store records the revision its
/// history starts from, derived once from a history that must already be consistent.
fn prepare_write_blocking(connection: &Connection, path: &Path) -> Result<Layout, StoreError> {
    let found = schema::check_blocking(connection, path)?;
    match found {
        Layout::Empty => schema::create_blocking(connection, path)?,
        Layout::Originless => {
            let origin = derived_origin_blocking(connection, path)?;
            schema::upgrade_blocking(connection, path)?;
            if let Some(origin) = origin {
                snapshot::write_origin_blocking(connection, path, origin)?;
            }
        }
        Layout::Current => {}
    }
    Ok(found)
}

fn derived_origin_blocking(
    connection: &Connection,
    path: &Path,
) -> Result<Option<u64>, StoreError> {
    let Some(plan) = load_blocking(connection, path)? else {
        return Ok(None);
    };
    let history = integrity::check_history_blocking(connection, path, plan.revision, None)?;
    Ok(Some(history.first_base_revision.unwrap_or(plan.revision)))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
