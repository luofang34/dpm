use std::path::{Path, PathBuf};
use thiserror::Error;

/// Project discovery and locator initialization failures.
#[derive(Debug, Error)]
pub enum ProjectError {
    /// No project locator was found before the filesystem or Git boundary.
    #[error(
        "no DPM project found from {start}; run dpm init in a project directory, or use --project DIR / --database PATH"
    )]
    NotFound {
        /// Canonical directory at which discovery started.
        start: PathBuf,
    },
    /// Locator syntax could not be parsed.
    #[error("invalid project locator {path}: {source}")]
    Parse {
        /// Locator path.
        path: PathBuf,
        /// Original TOML diagnostic.
        #[source]
        source: toml::de::Error,
    },
    /// Locator content violates the versioned contract.
    #[error("invalid project configuration at {path}: {message}")]
    Invalid {
        /// Directory or locator with the problem.
        path: PathBuf,
        /// Specific validation failure.
        message: String,
    },
    /// An existing project must not be overwritten during initialization.
    #[error("DPM project already exists at {path}; initialization never overwrites it")]
    AlreadyExists {
        /// Occupied project directory.
        path: PathBuf,
    },
    /// A legacy workspace requires an explicit choice instead of silent shadowing.
    #[error(
        "existing workspace at {path}; open it with dpm --database PATH, or configure .dpm/project.toml to reference it"
    )]
    Legacy {
        /// Existing database path.
        path: PathBuf,
    },
    /// Filesystem failure, with its original cause.
    #[error("{action} at {path}: {source}")]
    Io {
        /// Operation attempted.
        action: &'static str,
        /// Affected path.
        path: PathBuf,
        /// Filesystem diagnostic.
        #[source]
        source: std::io::Error,
    },
}

impl ProjectError {
    /// Stable category for adapter diagnostics.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "project_not_found",
            Self::AlreadyExists { .. } => "project_exists",
            Self::Legacy { .. } => "legacy_workspace",
            Self::Parse { .. } | Self::Invalid { .. } | Self::Io { .. } => "project_configuration",
        }
    }
}

pub(super) fn io_error(
    action: &'static str,
    path: &Path,
) -> impl FnOnce(std::io::Error) -> ProjectError {
    let path = path.to_path_buf();
    move |source| ProjectError::Io {
        action,
        path,
        source,
    }
}
