use crate::{Key, Plan, ResourceId, ValidationError, validation::invalid};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Shared resource identity independent of local paths and remote addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    /// Stable identity matching its map key.
    pub id: ResourceId,
    /// Human-readable key unique within the workspace's resources.
    pub key: Key,
    /// Human-readable resource label.
    pub label: String,
    /// Kind of resource; none grants evidence or task completion.
    pub kind: ResourceKind,
}

/// Resource category supporting both code and non-code work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceKind {
    /// A repository; remote URLs are attributes, not its identity.
    GitRepository {
        /// Public remote addresses without credentials.
        remotes: Vec<String>,
    },
    /// A filesystem collection with device-local location bindings.
    Folder,
    /// Documents used independently of a Git repository.
    DocumentCollection,
    /// Author-defined resource category.
    Other(String),
}

/// The access a task needs; this describes scope, not an authorization grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceAccess {
    /// Consume the resource without changing it.
    Read,
    /// Change the resource as part of the task contract.
    Write,
}

/// One explicit resource needed to perform a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRequirement {
    /// Resource in the workspace registry.
    pub resource: ResourceId,
    /// Required access; one entry per resource.
    pub access: ResourceAccess,
}

pub(crate) fn validate(plan: &Plan) -> Result<(), ValidationError> {
    let mut keys = BTreeSet::new();
    for (id, r) in &plan.resources {
        if id != &r.id
            || r.key.0.trim().is_empty()
            || r.key.0.trim() != r.key.0
            || !keys.insert(&r.key)
            || r.label.trim().is_empty()
        {
            return Err(invalid(
                "resource",
                id,
                "resource id, unique key and label are required",
            ));
        }
        match &r.kind {
            ResourceKind::Other(name) if name.trim().is_empty() => {
                return Err(invalid("resource kind", id, "category cannot be empty"));
            }
            ResourceKind::GitRepository { remotes }
                if remotes.iter().any(|r| r.trim().is_empty()) =>
            {
                return Err(invalid("resource remote", id, "address cannot be empty"));
            }
            _ => {}
        }
    }
    for work in plan.work_items.values() {
        let mut seen = BTreeSet::new();
        for requirement in &work.resources {
            if !plan.resources.contains_key(&requirement.resource)
                || !seen.insert(requirement.resource)
            {
                return Err(invalid(
                    "work resource",
                    work.id,
                    format!("missing or repeated resource {}", requirement.resource),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
