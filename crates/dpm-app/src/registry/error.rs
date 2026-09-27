use dpm_model::WorkspaceId;
use std::path::PathBuf;
use thiserror::Error;

/// Local registry failures, independent of workspace operations and revisions.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// Configuration must have an explicit absolute device-local location.
    #[error("set DPM_CONFIG_DIR, XDG_CONFIG_HOME or HOME to an absolute configuration directory")]
    ConfigDirectory,
    /// A registry file could not be read or created.
    #[error("workspace registry I/O at {path}: {source}")]
    Io {
        /// Affected local path.
        path: PathBuf,
        /// Original I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// Registry transaction or query failed.
    #[error("workspace registry at {path}: {source}")]
    Sqlite {
        /// Registry file.
        path: PathBuf,
        /// Original SQLite failure.
        #[source]
        source: rusqlite::Error,
    },
    /// Registry identity data is corrupt.
    #[error("invalid workspace identity at {path}: {source}")]
    Identity {
        /// Registry file.
        path: PathBuf,
        /// Original identity decoding failure.
        #[source]
        source: serde_json::Error,
    },
    /// Missing explicit association; reading must not initialize a new workspace.
    #[error("workspace {workspace} is not bound; run dpm workspace register --database PATH")]
    NotBound {
        /// Requested workspace.
        workspace: WorkspaceId,
    },
    /// Existing association must not be silently redirected.
    #[error(
        "workspace {workspace} is already bound to another store; use --replace to redirect this device"
    )]
    AlreadyBound {
        /// Conflicting workspace.
        workspace: WorkspaceId,
    },
    /// One file holds one store, so a second identity must not be bound to an occupied path.
    #[error(
        "{path} is bound to workspace {workspace}, but the store there contains {store}; use --replace to rebind the path"
    )]
    PathBound {
        /// Occupied database path.
        path: PathBuf,
        /// Identity already bound to the path.
        workspace: WorkspaceId,
        /// Identity the store at the path actually contains.
        store: WorkspaceId,
    },
    /// The bound file no longer exists; opening it would otherwise fail as a raw storage error.
    #[error(
        "workspace {workspace} is bound to {path}, which does not exist; run dpm workspace register --replace --database PATH"
    )]
    StoreMissing {
        /// Requested workspace.
        workspace: WorkspaceId,
        /// Missing bound path.
        path: PathBuf,
    },
    /// The bound file now holds another workspace, so the binding no longer names its store.
    #[error(
        "workspace {workspace} is bound to {path}, but that store contains workspace {found}; run dpm workspace register --replace --database PATH"
    )]
    IdentityMismatch {
        /// Requested workspace.
        workspace: WorkspaceId,
        /// Bound path.
        path: PathBuf,
        /// Identity found in the store.
        found: WorkspaceId,
    },
    /// Binding storage requires an unambiguous Unicode path.
    #[error("database path is not Unicode: {path}")]
    NonUnicode {
        /// Rejected local path.
        path: PathBuf,
    },
}
impl RegistryError {
    /// Stable diagnostic code shared by local adapters.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotBound { .. } => "workspace_not_bound",
            Self::AlreadyBound { .. } => "workspace_already_bound",
            Self::PathBound { .. } => "workspace_path_bound",
            Self::StoreMissing { .. } => "workspace_store_missing",
            Self::IdentityMismatch { .. } => "workspace_identity_mismatch",
            _ => "workspace_registry",
        }
    }
}
