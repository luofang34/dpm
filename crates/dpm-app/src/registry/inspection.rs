//! Health of each device binding, so a stale or ambiguous association is reported with a stable
//! status instead of surfacing later as an unrelated storage failure.

use super::{RegistryError, WorkspaceBinding, WorkspaceRegistry};
use crate::{AppError, Application};
use dpm_model::WorkspaceId;
use serde::Serialize;
use std::path::Path;

/// What the bound path holds now, read without creating or migrating a store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StoreState {
    /// The store exists and contains the bound workspace.
    Ok,
    /// Nothing exists at the bound path.
    Missing,
    /// The store contains another workspace.
    IdentityMismatch {
        /// Workspace identity found in the store.
        found: WorkspaceId,
    },
    /// The path exists but could not be read as an initialized store.
    Unreadable {
        /// Stable code of the failure that opening it would return.
        code: &'static str,
        /// Human-readable failure.
        message: String,
    },
}

/// One binding with the state of its store and any other identity bound to the same path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BindingReport {
    /// Registered association.
    #[serde(flatten)]
    pub binding: WorkspaceBinding,
    /// Current contents of the bound path.
    pub store: StoreState,
    /// Other workspaces bound to the same path; one file holds one store, so all but one are stale.
    pub shared_with: Vec<WorkspaceId>,
}

impl WorkspaceRegistry {
    /// List bindings with the state of each store, opening stores read-only when they exist.
    pub fn inspect_blocking(&self) -> Result<Vec<BindingReport>, RegistryError> {
        let bindings = self.list_blocking()?;
        Ok(bindings
            .iter()
            .map(|binding| BindingReport {
                store: store_state_blocking(binding),
                shared_with: bindings
                    .iter()
                    .filter(|other| {
                        other.database == binding.database && other.workspace != binding.workspace
                    })
                    .map(|other| other.workspace)
                    .collect(),
                binding: binding.clone(),
            })
            .collect())
    }
}

fn store_state_blocking(binding: &WorkspaceBinding) -> StoreState {
    match open_bound_blocking(binding.workspace, &binding.database) {
        Ok(_) => StoreState::Ok,
        Err(AppError::Registry(RegistryError::StoreMissing { .. })) => StoreState::Missing,
        Err(AppError::Registry(RegistryError::IdentityMismatch { found, .. })) => {
            StoreState::IdentityMismatch { found }
        }
        Err(error) => StoreState::Unreadable {
            code: error.code(),
            message: error.to_string(),
        },
    }
}

/// Open a bound store, reporting a missing file or a changed identity as registry states.
pub(crate) fn open_bound_blocking(
    workspace: WorkspaceId,
    path: &Path,
) -> Result<Application, AppError> {
    let exists = path.try_exists().map_err(|source| RegistryError::Io {
        path: path.into(),
        source,
    })?;
    if !exists {
        return Err(RegistryError::StoreMissing {
            workspace,
            path: path.into(),
        }
        .into());
    }
    let app = Application::open_blocking(path)?;
    let found = app.plan_blocking()?.workspace.id;
    if found != workspace {
        return Err(RegistryError::IdentityMismatch {
            workspace,
            path: path.into(),
            found,
        }
        .into());
    }
    Ok(app)
}
