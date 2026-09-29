//! SQLite snapshots and append-only operations committed as one validated transaction.
//!
//! The `sqlite` feature (default) builds the store itself. Without it the crate provides only the
//! records and errors the store returns, so an application layer built for a target without SQLite
//! keeps one result and error vocabulary.

mod error;
mod record;
#[cfg(feature = "sqlite")]
mod schema;
#[cfg(feature = "sqlite")]
mod sqlite;

pub use error::{LineageError, StoreError, StoredRecord};
pub use record::{HistoryEntry, HistoryPage, RecordedOperation, StoreLineage, StoreRevision};
#[cfg(feature = "sqlite")]
pub use schema::SCHEMA_VERSION;
#[cfg(feature = "sqlite")]
pub use sqlite::{
    IntegrityReport, SqliteStore, restore_store_blocking, store_revision_blocking,
    verify_store_blocking,
};
