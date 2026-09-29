//! Shared query and mutation boundary for CLI and agent adapters.

mod authoring;
mod envelope;
mod error;
mod git_artifact;
mod interchange;
mod ownership;
mod project;
mod registry;
mod service;
mod tracking;
pub use registry::{BindingReport, RegistryError, StoreState, WorkspaceBinding, WorkspaceRegistry};
pub use tracking::ExternalLinkInput;

pub use project::{
    ProjectError, ProjectLocation, ProjectSource, initialize_project_blocking,
    open_workspace_blocking,
};

pub use authoring::plan_schema;
pub use dpm_interchange::ExistingMatch;
pub use dpm_store::IntegrityReport;
pub use envelope::{Envelope, PlanValidation, validate_plan};
pub use error::{AppError, ErrorResponse};
pub use service::{
    API_VERSION, Application, CommandRequest, PlanChangeRequest, Query, QueryResponse,
};
pub use service::{restore_store_blocking, store_path_blocking, verify_store_blocking};
