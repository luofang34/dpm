use serde::Serialize;
use thiserror::Error;

/// Shared application failures, preserving the original engine or storage cause.
#[derive(Debug, Error)]
pub enum AppError {
    /// Local workspace binding failed.
    #[error(transparent)]
    Registry(#[from] crate::RegistryError),
    /// Invalid or missing project locator.
    #[error(transparent)]
    Project(#[from] crate::ProjectError),
    /// Preview sources do not authorize persistence or task operations.
    #[error(
        "project is a read-only preview; import the plan into a separate workspace before operating on it"
    )]
    ReadOnlyProject,
    /// Domain query or command rejected.
    #[error(transparent)]
    Engine(#[from] dpm_engine::EngineError),
    /// Query scope names an entity absent from the plan.
    #[error(transparent)]
    Scope(#[from] dpm_engine::ScopeError),
    /// Persistence failed.
    #[error(transparent)]
    Store(#[from] dpm_store::StoreError),
    /// Structured request or response is invalid.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Git process could not be launched.
    #[error("run git: {0}")]
    GitIo(#[source] std::io::Error),
    /// Git command failed.
    #[error("git {args} failed: {message}")]
    Git {
        /// Read-only Git arguments.
        args: String,
        /// Git diagnostic.
        message: String,
    },
    /// Git output was not valid UTF-8.
    #[error("git output is not UTF-8: {0}")]
    GitEncoding(#[from] std::string::FromUtf8Error),
    /// Database exists but contains no initialized plan.
    #[error("workspace is not initialized")]
    NotInitialized,
    /// Caller based a command on an outdated snapshot.
    #[error("revision conflict: expected {expected}, current {actual}")]
    Conflict {
        /// Revision the caller observed.
        expected: u64,
        /// Revision currently stored.
        actual: u64,
    },
    /// Request parameter is absent or invalid.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// Work key is not in the current plan.
    #[error("unknown work key {0}")]
    UnknownWork(String),
    /// Decision key is not in the current plan.
    #[error("unknown decision key {0}")]
    UnknownDecision(String),
}

/// Stable machine diagnostic shared by adapters.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    /// Application contract version.
    pub api_version: u32,
    /// Stable error category; clients must not parse the message.
    pub code: &'static str,
    /// Human-readable context.
    pub message: String,
}
impl AppError {
    /// Stable category for coding agents.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Project(error) => error.code(),
            Self::Registry(error) => error.code(),
            Self::ReadOnlyProject => "read_only_project",
            Self::Conflict { .. }
            | Self::Engine(dpm_engine::EngineError::RevisionConflict { .. })
            | Self::Store(dpm_store::StoreError::RevisionConflict { .. }) => "revision_conflict",
            Self::GitIo(_) | Self::Git { .. } | Self::GitEncoding(_) => "git_error",
            Self::NotInitialized => "not_initialized",
            Self::UnknownWork(_) | Self::UnknownDecision(_) | Self::Scope(_) => "not_found",
            Self::Engine(_) => "invalid_command",
            Self::Store(_) => "storage_error",
            Self::Json(_) | Self::InvalidRequest(_) => "invalid_request",
        }
    }
    /// Serialize a diagnostic without presentation-specific parsing.
    pub fn response(&self) -> ErrorResponse {
        ErrorResponse {
            api_version: crate::API_VERSION,
            code: self.code(),
            message: self.to_string(),
        }
    }
}
