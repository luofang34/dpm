use crate::{
    StoreError,
    error::database_error,
    schema::{self, Layout},
};
use dpm_engine::{Operation, apply_command};
use dpm_model::Plan;
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

mod history;
mod integrity;
mod recovery;
use recovery::files;
mod replay;
pub use history::{HistoryEntry, HistoryPage};
pub use recovery::{IntegrityReport, restore_store_blocking, verify_store_blocking};
mod snapshot;
mod snapshot_cache;
use snapshot::load_blocking;

/// A synchronous local database with validated snapshots and immutable operation history.
pub struct SqliteStore {
    connection: Connection,
    cache: snapshot_cache::SnapshotCache,
    path: PathBuf,
}

impl SqliteStore {
    /// Create or open a database without writing to it; [`Self::initialize_blocking`] creates the
    /// schema.
    pub fn open_blocking(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        check_existing_read_only_blocking(&path)?;
        let connection = Connection::open(&path).map_err(database_error(&path, "open database"))?;
        Self::prepare_blocking(connection, path)
    }

    /// Open an existing database without creating a new file or writing to it; a store in a
    /// retired or newer layout is refused unchanged.
    pub fn open_existing_blocking(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        check_existing_read_only_blocking(&path)?;
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
        // Refused formats are checked before this read-write connection opens; schema creation
        // happens inside the initializing write transaction.
        schema::check_blocking(&connection, &path)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(database_error(&path, "configure database"))?;
        Ok(Self {
            connection,
            path,
            cache: Default::default(),
        })
    }

    /// Read and validate the authoritative snapshot, if initialized.
    pub fn load_blocking(&self) -> Result<Option<Plan>, StoreError> {
        if schema::check_blocking(&self.connection, &self.path)? == Layout::Empty {
            return Ok(None);
        }
        snapshot::load_cached_blocking(&self.connection, &self.path, &self.cache)
    }

    /// Initialize an empty database with `plan` as both the snapshot and the genesis plan the
    /// history replays from; an existing snapshot or operation history is never erased, and a
    /// refused initialization writes nothing.
    pub fn initialize_blocking(&mut self, plan: &Plan) -> Result<(), StoreError> {
        plan.validate()?;
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
        snapshot::write_initial_blocking(&transaction, &self.path, plan)?;
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
        let mut expected = snapshot::load_cached_blocking(&transaction, &self.path, &self.cache)?
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

/// Check the version and layout of an existing file through a read-only connection first.
///
/// Closing the last read-write connection to a WAL store checkpoints the log into the main file and
/// deletes the side files, so a store this binary refuses must never be opened read-write at all.
fn check_existing_read_only_blocking(path: &Path) -> Result<(), StoreError> {
    let exists = path.try_exists().map_err(|source| StoreError::Io {
        action: "inspect",
        path: path.to_path_buf(),
        source,
    })?;
    if !exists {
        return Ok(());
    }
    let mode = files::read_mode_blocking(path)?;
    let connection = files::open_read_only_blocking(path, mode)?;
    if schema::check_blocking(&connection, path)? == Layout::Current {
        snapshot::load_blocking(&connection, path)?;
        snapshot::genesis_blocking(&connection, path)?;
    }
    Ok(())
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

/// Create the schema of an empty store inside a caller-owned write transaction and return the
/// layout found before that.
///
/// The exact layout is re-checked under the write lock, so a schema object added by another
/// process after open never runs inside this write.
fn prepare_write_blocking(connection: &Connection, path: &Path) -> Result<Layout, StoreError> {
    let found = schema::check_blocking(connection, path)?;
    if found == Layout::Empty {
        schema::create_blocking(connection, path)?;
    }
    Ok(found)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
