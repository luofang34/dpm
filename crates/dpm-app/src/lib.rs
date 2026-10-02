//! Shared query and mutation boundary for CLI, agent and native adapters.
//!
//! Every feature is on by default. A client target may leave out `sqlite` (local stores),
//! `registry` (device-local workspace bindings, which needs `sqlite`) and `git` (HEAD evidence
//! captured through the git command-line tool). Without `sqlite` an [`Application`] opens
//! read-only previews: queries answer, and mutations are refused as read-only. A request that needs
//! a left-out feature fails with [`AppError::Unsupported`].
//!
//! [`Application`] is `Send`, not `Sync`; see its documentation for how clients share it.

mod authoring;
mod envelope;
mod error;
#[cfg(feature = "git")]
mod git_artifact;
mod interchange;
mod ownership;
mod project;
#[cfg(feature = "registry")]
mod registry;
mod service;
mod tracking;
#[cfg(feature = "registry")]
pub use registry::{BindingReport, RegistryError, StoreState, WorkspaceBinding, WorkspaceRegistry};
pub use tracking::ExternalLinkInput;

#[cfg(feature = "sqlite")]
pub use project::initialize_project_blocking;
pub use project::{ProjectError, ProjectLocation, ProjectSource, open_workspace_blocking};

pub use authoring::plan_schema;
pub use dpm_interchange::ExistingMatch;
#[cfg(feature = "sqlite")]
pub use dpm_store::IntegrityReport;
pub use dpm_store::{HistoryPage, RecordedOperation};
pub use envelope::{Envelope, PlanValidation, validate_decoded, validate_plan};
pub use error::{AppError, ErrorResponse};
pub use service::{
    API_VERSION, ActivityReceipt, Application, CommandRequest, NextRequest, Observed,
    PlanChangeRequest, Query, QueryClock, QueryResponse, ReceivedActivity, RunActivityRequest,
    RunCommand, RunLinkRequest, RunList, RunQuery, RunReportRequest, RunStartRequest, RunWrite,
    StatusView, WorkDetail, WorkspaceRevision,
};
#[cfg(feature = "sqlite")]
pub use service::{restore_store_blocking, store_path_blocking, verify_store_blocking};
