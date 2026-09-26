//! Shared query and mutation boundary for CLI and agent adapters.

mod error;
mod git_artifact;
mod project;
mod service;

pub use project::{
    ProjectError, ProjectLocation, ProjectSource, initialize_project_blocking,
    open_workspace_blocking,
};

pub use error::{AppError, ErrorResponse};
pub use service::{API_VERSION, Application, CommandRequest, Query, QueryResponse};

pub use git_artifact::git_head_artifact_blocking;
