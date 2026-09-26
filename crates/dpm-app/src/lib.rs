//! Shared query and mutation boundary for CLI and agent adapters.

mod error;
mod git_artifact;
mod interchange;
mod project;
mod registry;
mod service;
mod tracking;
pub use registry::{RegistryError, WorkspaceBinding, WorkspaceRegistry};
pub use tracking::ExternalLinkInput;

pub use project::{
    ProjectError, ProjectLocation, ProjectSource, initialize_project_blocking,
    open_workspace_blocking,
};

pub use error::{AppError, ErrorResponse};
pub use service::{API_VERSION, Application, CommandRequest, Query, QueryResponse};
