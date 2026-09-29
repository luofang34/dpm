//! Opening and creating authoritative SQLite stores.

use super::{Application, Backing};
use crate::AppError;
use dpm_model::Plan;
use dpm_store::SqliteStore;
use std::path::Path;

impl Application {
    /// Initialize local authoritative state through the shared application boundary.
    pub fn initialize_blocking(path: &Path, plan: &Plan) -> Result<Self, AppError> {
        plan.validate().map_err(dpm_store::StoreError::from)?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| crate::ProjectError::Io {
                action: "create database directory",
                path: parent.into(),
                source,
            })?;
        }
        let mut store = SqliteStore::open_blocking(path)?;
        store.initialize_blocking(plan)?;
        Ok(Self::with_backing(Backing::Database(store)))
    }
    /// Open an existing database without accidentally creating a workspace on a read request.
    pub fn open_blocking(path: impl AsRef<Path>) -> Result<Self, AppError> {
        let store = SqliteStore::open_existing_blocking(path)?;
        Ok(Self::with_backing(Backing::Database(store)))
    }
    /// Create a disposable workspace for tests or embedded clients.
    pub fn in_memory_blocking(plan: &Plan) -> Result<Self, AppError> {
        let mut store = SqliteStore::in_memory_blocking()?;
        store.initialize_blocking(plan)?;
        Ok(Self::with_backing(Backing::Database(store)))
    }
}
