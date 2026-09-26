//! SQLite snapshots and append-only operations committed as one validated transaction.

mod error;
mod sqlite;

pub use error::StoreError;
pub use sqlite::{HistoryEntry, HistoryPage, SqliteStore};
