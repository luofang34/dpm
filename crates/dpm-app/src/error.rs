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
    /// A project file could not be mapped to or from the supported interchange subset.
    #[error(transparent)]
    Interchange(#[from] dpm_interchange::InterchangeError),
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
    /// No external reference with this canonical identity is recorded.
    #[error("unknown external reference {0}")]
    UnknownExternalReference(String),
    /// Dependency identity is malformed or not in the current plan.
    #[error("unknown dependency {0}")]
    UnknownDependency(String),
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
    /// Structured context for refusals an agent can act on, such as the unmet gates of a transition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
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
            Self::UnknownWork(_)
            | Self::UnknownDecision(_)
            | Self::UnknownDependency(_)
            | Self::UnknownExternalReference(_)
            | Self::Scope(_)
            | Self::Engine(
                dpm_engine::EngineError::MissingExternalReference(_)
                | dpm_engine::EngineError::MissingExternalLink { .. },
            ) => "not_found",
            Self::Engine(dpm_engine::EngineError::TrackingOwned { .. }) => "tracking_conflict",
            Self::Engine(_) => "invalid_command",
            Self::Interchange(error) => error.code(),
            Self::Store(dpm_store::StoreError::UnsupportedSchemaVersion { .. }) => {
                "unsupported_schema_version"
            }
            Self::Store(dpm_store::StoreError::TargetExists { .. }) => "target_exists",
            Self::Store(error) if error.is_corruption() => "corrupt_store",
            Self::Store(_) => "storage_error",
            Self::Json(_) | Self::InvalidRequest(_) => "invalid_request",
        }
    }
    /// Structured refusal context: a refused transition returns the same unmet gates as `explain`.
    pub fn details(&self) -> Option<serde_json::Value> {
        match self {
            Self::Engine(dpm_engine::EngineError::NotReady {
                transition, unmet, ..
            }) => Some(serde_json::json!({"transition": transition, "unmet": unmet})),
            _ => None,
        }
    }
    /// Serialize a diagnostic without presentation-specific parsing.
    pub fn response(&self) -> ErrorResponse {
        ErrorResponse {
            api_version: crate::API_VERSION,
            code: self.code(),
            message: self.to_string(),
            details: self.details(),
        }
    }
}
