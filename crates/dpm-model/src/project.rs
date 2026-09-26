use crate::{Key, ProjectId, WorkspaceId};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Top-level namespace for a plan.
pub struct Workspace {
    /// Stable entity identity; it must match its containing map key.
    pub id: WorkspaceId,
    /// Non-empty local name.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Project objective, optionally nested under another project.
pub struct Project {
    /// Stable entity identity; it must match its containing map key.
    pub id: ProjectId,
    /// Human-readable key, unique in this entity category.
    pub key: Key,
    /// Optional parent; containment must be acyclic.
    pub parent: Option<ProjectId>,
    /// Non-empty human-readable title.
    pub title: String,
    /// Observable purpose of this work.
    pub objective: String,
}
