use dpm_store::LineageError;
use serde::Serialize;
use thiserror::Error;

/// Shared application failures, preserving the original engine or storage cause.
#[derive(Debug, Error)]
pub enum AppError {
    /// Local workspace binding failed.
    #[cfg(feature = "registry")]
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
    /// A decoded plan breaks a graph invariant.
    #[error(transparent)]
    Validation(#[from] dpm_model::ValidationError),
    /// Structured request or response is invalid.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Git process could not be launched.
    #[cfg(feature = "git")]
    #[error("run git: {0}")]
    GitIo(#[source] std::io::Error),
    /// Git command failed.
    #[cfg(feature = "git")]
    #[error("git {args} failed: {message}")]
    Git {
        /// Read-only Git arguments.
        args: String,
        /// Git diagnostic.
        message: String,
    },
    /// Git output was not valid UTF-8.
    #[cfg(feature = "git")]
    #[error("git output is not UTF-8: {0}")]
    GitEncoding(#[from] std::string::FromUtf8Error),
    /// This build leaves out the feature the request needs, such as a SQLite store on a client
    /// target that only opens previews.
    #[error("this build of DPM leaves out the {feature} feature")]
    Unsupported {
        /// Cargo feature of `dpm-app` the request needs.
        feature: &'static str,
    },
    /// Database exists but contains no initialized plan.
    #[error("workspace is not initialized")]
    NotInitialized,
    /// An operation identity is already recorded for a different actor or content.
    #[error(
        "operation {} is already recorded with different content; send a new operation id for a new change",
        .recorded.operation.id
    )]
    DuplicateOperation {
        /// The operation recorded under this identity.
        recorded: Box<dpm_store::RecordedOperation>,
    },
    /// A run request was refused by the run store or could not be recorded safely.
    #[error(transparent)]
    RunStore(Box<dpm_store::RunStoreError>),
    /// A run request was refused by the engine.
    #[error(transparent)]
    Run(Box<dpm_engine::RunError>),
    /// No committed operation has this identity.
    #[error("unknown operation {0}")]
    UnknownOperation(dpm_model::OperationId),
    /// Caller based a command on an outdated snapshot.
    #[error("revision conflict: expected {expected}, current {actual}")]
    Conflict {
        /// Revision the caller observed.
        expected: u64,
        /// Revision currently stored.
        actual: u64,
    },
    /// A read of the snapshot together with its whole operation log never saw both at one
    /// revision, because other clients kept committing.
    #[error(
        "the workspace kept changing during the read: {reads} reads of the snapshot and its \
         operation log each saw a new commit; retry when writes pause"
    )]
    HistoryChanging {
        /// Reads attempted.
        reads: usize,
    },
    /// A principal argument is not spelled `KIND:NAME`.
    #[error(transparent)]
    Actor(#[from] dpm_model::ActorParseError),
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

impl From<dpm_store::RunStoreError> for AppError {
    fn from(error: dpm_store::RunStoreError) -> Self {
        Self::RunStore(Box::new(error))
    }
}

impl From<dpm_engine::RunError> for AppError {
    fn from(error: dpm_engine::RunError) -> Self {
        Self::Run(Box::new(error))
    }
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
            #[cfg(feature = "registry")]
            Self::Registry(error) => error.code(),
            Self::Unsupported { .. } => "unsupported",
            Self::ReadOnlyProject => "read_only_project",
            Self::Conflict { .. }
            | Self::HistoryChanging { .. }
            | Self::Engine(dpm_engine::EngineError::RevisionConflict { .. })
            | Self::Store(dpm_store::StoreError::RevisionConflict { .. }) => "revision_conflict",
            #[cfg(feature = "git")]
            Self::GitIo(_) | Self::Git { .. } | Self::GitEncoding(_) => "git_error",
            Self::DuplicateOperation { .. }
            | Self::Store(dpm_store::StoreError::DuplicateOperation { .. }) => {
                "duplicate_operation"
            }
            Self::Store(dpm_store::StoreError::Lineage(LineageError::Mismatch { .. })) => {
                "lineage_mismatch"
            }
            Self::Store(dpm_store::StoreError::Lineage(LineageError::Archived { .. })) => {
                "archived_store"
            }
            Self::Store(error) if error.is_busy() => "store_busy",
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
            Self::Store(
                dpm_store::StoreError::UnsupportedSchemaVersion { .. }
                | dpm_store::StoreError::RetiredSchemaVersion { .. },
            ) => "unsupported_schema_version",
            Self::RunStore(error) => run_store_code(error),
            Self::Run(error) => run_code(error),
            Self::UnknownOperation(_) => "not_found",
            Self::Store(dpm_store::StoreError::TargetExists { .. }) => "target_exists",
            Self::Store(
                dpm_store::StoreError::InvalidTarget { .. }
                | dpm_store::StoreError::Lineage(LineageError::NotAnArchive { .. }),
            ) => "invalid_request",
            Self::Store(error) if error.is_corruption() => "corrupt_store",
            Self::Store(_) => "storage_error",
            Self::Validation(_) => "invalid_plan",
            Self::Json(_) | Self::InvalidRequest(_) | Self::Actor(_) => "invalid_request",
        }
    }
    /// Structured refusal context: a refused transition returns the same unmet gates as `explain`.
    pub fn details(&self) -> Option<serde_json::Value> {
        match self {
            Self::Engine(dpm_engine::EngineError::NotReady {
                transition, unmet, ..
            }) => Some(serde_json::json!({"transition": transition, "unmet": unmet})),
            Self::Engine(dpm_engine::EngineError::OwnGateRelaxed { work, relaxed, .. }) => {
                Some(serde_json::json!({"work": work, "relaxed": relaxed}))
            }
            Self::DuplicateOperation { recorded } => {
                Some(serde_json::json!({"recorded": recorded}))
            }
            Self::Store(dpm_store::StoreError::Lineage(LineageError::Mismatch {
                expected,
                actual,
            })) => Some(serde_json::json!({"expected": expected, "actual": actual})),
            Self::RunStore(error) => run_store_details(error),
            Self::Run(error) => match error.as_ref() {
                dpm_engine::RunError::LinkRefused { reason, .. } => {
                    Some(serde_json::json!({"reason": reason.to_string()}))
                }
                _ => None,
            },
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

/// Stable category of a run store refusal; the same words the project store's refusals use for the
/// same causes, so one client handles busy, lineage and corruption alike.
fn run_store_code(error: &dpm_store::RunStoreError) -> &'static str {
    use dpm_store::RunStoreError as Refused;
    match error {
        Refused::Store(dpm_store::StoreError::Lineage(LineageError::Mismatch { .. })) => {
            "lineage_mismatch"
        }
        Refused::Store(dpm_store::StoreError::Lineage(LineageError::Archived { .. })) => {
            "archived_store"
        }
        _ if error.is_busy() => "store_busy",
        Refused::Store(
            dpm_store::StoreError::UnsupportedSchemaVersion { .. }
            | dpm_store::StoreError::RetiredSchemaVersion { .. },
        ) => "unsupported_schema_version",
        _ if error.is_corruption() => "corrupt_store",
        Refused::Store(_) => "storage_error",
        Refused::Run(error) => run_code(error),
        Refused::UnknownRun(_) | Refused::UnknownParent(_) => "not_found",
        Refused::DuplicateRun { .. }
        | Refused::DuplicateEvent { .. }
        | Refused::DuplicateActivity { .. } => "duplicate_run_record",
        Refused::LinkConflict { .. } => "run_link_conflict",
        Refused::ActivityExpired { .. } => "activity_expired",
        Refused::ForeignRun { .. } => "foreign_run",
        Refused::WorkspaceMismatch { .. } => "workspace_mismatch",
        Refused::Corrupt { .. } => "corrupt_store",
    }
}

fn run_code(error: &dpm_engine::RunError) -> &'static str {
    use dpm_engine::RunError as Refused;
    match error {
        Refused::Validation(_) => "invalid_request",
        Refused::MissingWork(_) => "not_found",
        Refused::InvalidTransition { .. } | Refused::Terminal { .. } => "invalid_run_transition",
        Refused::LinkRefused { .. } => "run_link_refused",
        Refused::NotATask(_)
        | Refused::NotExecuting { .. }
        | Refused::NotOwner { .. }
        | Refused::ActorNotAllowed { .. }
        | Refused::ManagedNeedsService(_)
        | Refused::UnsupportedSource(_) => "invalid_command",
    }
}

fn run_store_details(error: &dpm_store::RunStoreError) -> Option<serde_json::Value> {
    use dpm_store::RunStoreError as Refused;
    match error {
        Refused::Store(dpm_store::StoreError::Lineage(LineageError::Mismatch {
            expected,
            actual,
        })) => Some(serde_json::json!({"expected": expected, "actual": actual})),
        Refused::DuplicateRun { recorded } => Some(serde_json::json!({"recorded": recorded})),
        Refused::DuplicateEvent { recorded } => Some(serde_json::json!({"recorded": recorded})),
        Refused::DuplicateActivity { recorded } => Some(serde_json::json!({"recorded": recorded})),
        Refused::LinkConflict { operation, linked } => {
            Some(serde_json::json!({"operation": operation, "linked": linked}))
        }
        Refused::ActivityExpired {
            run,
            source_sequence,
            high_water,
        } => Some(
            serde_json::json!({"run": run, "source_sequence": source_sequence, "high_water": high_water}),
        ),
        Refused::ForeignRun {
            run,
            recorded,
            current,
        } => Some(serde_json::json!({"run": run, "recorded": recorded, "current": current})),
        Refused::Run(dpm_engine::RunError::LinkRefused { reason, .. }) => {
            Some(serde_json::json!({"reason": reason.to_string()}))
        }
        _ => None,
    }
}
