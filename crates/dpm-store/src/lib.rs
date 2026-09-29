//! SQLite snapshots and append-only operations committed as one validated transaction.

mod error;
mod schema;
mod sqlite;

pub use error::{LineageError, StoreError, StoredRecord};
pub use schema::SCHEMA_VERSION;
pub use sqlite::{
    HistoryEntry, HistoryPage, IntegrityReport, RecordedOperation, SqliteStore, StoreLineage,
    restore_store_blocking, verify_store_blocking,
};
