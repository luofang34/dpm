use crate::{AppError, Application};
use dpm_model::WorkspaceId;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

mod error;
mod inspection;
pub use error::RegistryError;
pub(crate) use inspection::open_bound_blocking;
pub use inspection::{BindingReport, StoreState};

/// Device-local association, excluded from portable plans and repository locators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceBinding {
    /// Workspace identity checked against the selected store.
    pub workspace: WorkspaceId,
    /// Canonical local database path.
    pub database: PathBuf,
}

/// Local configuration service; it never mutates a workspace or creates its store.
pub struct WorkspaceRegistry {
    path: PathBuf,
}

impl WorkspaceRegistry {
    /// Select an explicit registry file, useful for isolated device configuration and tests.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Resolve centralized device configuration from DPM_CONFIG_DIR, XDG_CONFIG_HOME or HOME.
    pub fn from_environment() -> Result<Self, RegistryError> {
        let directory = if let Some(path) = std::env::var_os("DPM_CONFIG_DIR") {
            PathBuf::from(path)
        } else if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
            PathBuf::from(path).join("dpm")
        } else {
            PathBuf::from(std::env::var_os("HOME").ok_or(RegistryError::ConfigDirectory)?)
                .join(".config/dpm")
        };
        if !directory.is_absolute() {
            return Err(RegistryError::ConfigDirectory);
        }
        Ok(Self::at(directory.join("workspaces.sqlite")))
    }

    /// Register an existing store; replacement requires an explicit local configuration choice.
    pub fn register_blocking(
        &self,
        database: &Path,
        replace: bool,
    ) -> Result<WorkspaceBinding, AppError> {
        let database = fs::canonicalize(database).map_err(|source| RegistryError::Io {
            path: database.into(),
            source,
        })?;
        let workspace = Application::open_blocking(&database)?
            .plan_blocking()?
            .workspace
            .id;
        let text = database.to_str().ok_or_else(|| RegistryError::NonUnicode {
            path: database.clone(),
        })?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| RegistryError::Io {
                path: parent.into(),
                source,
            })?;
        }
        self.bind_blocking(workspace, text, replace)?;
        Ok(WorkspaceBinding {
            workspace,
            database,
        })
    }

    /// Store the association in one immediate transaction. A path holds exactly one store, so any
    /// other identity bound to it is stale; keeping it would let two workspaces resolve to one file.
    fn bind_blocking(
        &self,
        workspace: WorkspaceId,
        text: &str,
        replace: bool,
    ) -> Result<(), RegistryError> {
        let mut connection = Connection::open(&self.path).map_err(|source| self.sql(source))?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS bindings (workspace TEXT PRIMARY KEY, database_path TEXT NOT NULL);").map_err(|source| self.sql(source))?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|source| self.sql(source))?;
        let existing: Option<String> = transaction
            .query_row(
                "SELECT database_path FROM bindings WHERE workspace = ?1",
                [workspace.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| self.sql(source))?;
        if !replace && existing.as_deref().is_some_and(|p| p != text) {
            return Err(RegistryError::AlreadyBound { workspace });
        }
        let occupant: Option<String> = transaction
            .query_row(
                "SELECT workspace FROM bindings WHERE database_path = ?1 AND workspace <> ?2 ORDER BY workspace",
                params![text, workspace.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| self.sql(source))?;
        if let Some(bound) = occupant {
            if !replace {
                return Err(RegistryError::PathBound {
                    path: text.into(),
                    workspace: self.identity(bound)?,
                    store: workspace,
                });
            }
            transaction
                .execute(
                    "DELETE FROM bindings WHERE database_path = ?1 AND workspace <> ?2",
                    params![text, workspace.to_string()],
                )
                .map_err(|source| self.sql(source))?;
        }
        transaction.execute("INSERT INTO bindings VALUES (?1, ?2) ON CONFLICT(workspace) DO UPDATE SET database_path=excluded.database_path", params![workspace.to_string(), text]).map_err(|source| self.sql(source))?;
        transaction.commit().map_err(|source| self.sql(source))
    }

    /// Read registered locations without creating configuration files or opening the stores;
    /// [`Self::inspect_blocking`] adds the state of each store.
    pub fn list_blocking(&self) -> Result<Vec<WorkspaceBinding>, RegistryError> {
        let Some(connection) = self.open_read_blocking()? else {
            return Ok(Vec::new());
        };
        let mut statement = connection
            .prepare("SELECT workspace, database_path FROM bindings ORDER BY workspace")
            .map_err(|source| self.sql(source))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|source| self.sql(source))?;
        rows.map(|row| {
            let (id, database) = row.map_err(|source| self.sql(source))?;
            Ok(WorkspaceBinding {
                workspace: self.identity(id)?,
                database: database.into(),
            })
        })
        .collect()
    }

    /// Resolve a binding; the caller must verify that the opened store still has this identity.
    pub fn resolve_blocking(&self, workspace: WorkspaceId) -> Result<PathBuf, RegistryError> {
        self.list_blocking()?
            .into_iter()
            .find(|b| b.workspace == workspace)
            .map(|b| b.database)
            .ok_or(RegistryError::NotBound { workspace })
    }

    fn open_read_blocking(&self) -> Result<Option<Connection>, RegistryError> {
        match fs::metadata(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(RegistryError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
            Ok(_) => {}
        }
        Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map(Some)
            .map_err(|source| self.sql(source))
    }

    fn identity(&self, id: String) -> Result<WorkspaceId, RegistryError> {
        serde_json::from_value(serde_json::Value::String(id)).map_err(|source| {
            RegistryError::Identity {
                path: self.path.clone(),
                source,
            }
        })
    }

    fn sql(&self, source: rusqlite::Error) -> RegistryError {
        RegistryError::Sqlite {
            path: self.path.clone(),
            source,
        }
    }
}

#[cfg(test)]
mod tests;
