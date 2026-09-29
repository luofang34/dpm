use crate::{AssetId, Key, Plan, ValidationError, validation::invalid};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Shared asset identity independent of local paths and remote addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceAsset {
    /// Stable identity matching its map key.
    pub id: AssetId,
    /// Human-readable key unique within the workspace's assets.
    pub key: Key,
    /// Human-readable asset label.
    pub label: String,
    /// Kind of asset; none grants evidence or task completion.
    pub kind: AssetKind,
}

/// WorkspaceAsset category supporting both code and non-code work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum AssetKind {
    /// A repository; remote URLs are attributes, not its identity.
    GitRepository {
        /// Public remote addresses without credentials.
        remotes: Vec<String>,
    },
    /// A filesystem collection with device-local location bindings.
    Folder,
    /// Documents used independently of a Git repository.
    DocumentCollection,
    /// Author-defined asset category.
    Other(String),
}

/// The access a task needs; this describes scope, not an authorization grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetAccess {
    /// Consume the asset without changing it.
    Read,
    /// Change the asset as part of the task contract.
    Write,
}

/// One explicit asset needed to perform a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetRequirement {
    /// WorkspaceAsset in the workspace registry.
    pub asset: AssetId,
    /// Required access; one entry per asset.
    pub access: AssetAccess,
}

pub(crate) fn validate(plan: &Plan) -> Result<(), ValidationError> {
    let mut keys = BTreeSet::new();
    for (id, r) in &plan.assets {
        if id != &r.id
            || r.key.0.trim().is_empty()
            || r.key.0.trim() != r.key.0
            || !keys.insert(&r.key)
            || r.label.trim().is_empty()
        {
            return Err(invalid(
                "asset",
                id,
                "asset id, unique key and label are required",
            ));
        }
        match &r.kind {
            AssetKind::Other(name) if name.trim().is_empty() => {
                return Err(invalid("asset kind", id, "category cannot be empty"));
            }
            AssetKind::GitRepository { remotes } if remotes.iter().any(|r| r.trim().is_empty()) => {
                return Err(invalid("asset remote", id, "address cannot be empty"));
            }
            AssetKind::GitRepository { remotes }
                if remotes
                    .iter()
                    .any(|r| crate::credentials::check_remote(r).is_err()) =>
            {
                return Err(invalid(
                    "asset remote",
                    id,
                    "shared plans cannot carry remote credentials",
                ));
            }
            _ => {}
        }
    }
    for work in plan.work_items.values() {
        let mut seen = BTreeSet::new();
        for requirement in &work.contract.assets {
            if !plan.assets.contains_key(&requirement.asset) || !seen.insert(requirement.asset) {
                return Err(invalid(
                    "work asset",
                    work.id,
                    format!("missing or repeated asset {}", requirement.asset),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
