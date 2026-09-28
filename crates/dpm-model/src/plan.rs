mod decode;

use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Serialize)]
/// Authoritative graph and revision; scheduling and completion views are derived.
#[serde(deny_unknown_fields)]
pub struct Plan {
    /// Portable domain format version; unsupported formats are rejected.
    #[serde(deserialize_with = "read_format_version")]
    pub format_version: u32,
    /// Assets shared across code and non-code work.
    pub assets: BTreeMap<AssetId, WorkspaceAsset>,
    /// Namespace containing the graph.
    pub workspace: Workspace,
    /// Wrapping operation sequence for optimistic concurrency.
    pub revision: u64,
    /// Projects indexed by stable identifier.
    pub projects: BTreeMap<ProjectId, Project>,
    /// Work indexed by stable identifier.
    pub work_items: BTreeMap<WorkItemId, WorkItem>,
    /// Requirements indexed by stable identifier.
    pub requirements: BTreeMap<RequirementId, Requirement>,
    /// Evidence indexed by stable identifier.
    pub artifacts: BTreeMap<ArtifactId, Artifact>,
    /// Decision gates indexed by stable identifier.
    pub decisions: BTreeMap<DecisionId, Decision>,
    /// Risks indexed by stable identifier.
    pub risks: BTreeMap<RiskId, Risk>,
    /// Directed temporal constraints; cycles are rejected.
    pub dependencies: Vec<Dependency>,
    /// External tracker objects linked to work; context only, never evidence or gates.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub external_references: BTreeMap<ExternalReferenceId, ExternalReference>,
    /// Contextual work relationships that never gate readiness, scheduling or progress.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<WorkLink>,
}

impl Plan {
    /// Create an empty namespace at revision zero.
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            format_version: 3,
            assets: BTreeMap::new(),
            workspace: Workspace {
                id: WorkspaceId::new(),
                name: name.into(),
            },
            revision: 0,
            projects: BTreeMap::new(),
            work_items: BTreeMap::new(),
            requirements: BTreeMap::new(),
            artifacts: BTreeMap::new(),
            decisions: BTreeMap::new(),
            risks: BTreeMap::new(),
            dependencies: Vec::new(),
            external_references: BTreeMap::new(),
            links: Vec::new(),
        }
    }

    /// Find authoritative work by its human-readable key.
    pub fn find_work_by_key(&self, key: &str) -> Option<&WorkItem> {
        self.work_items.values().find(|work| work.key.0 == key)
    }

    /// Mutably find authoritative work; callers must validate edits.
    pub fn find_work_by_key_mut(&mut self, key: &str) -> Option<&mut WorkItem> {
        self.work_items.values_mut().find(|work| work.key.0 == key)
    }

    /// Find a temporal constraint by its stable identity.
    pub fn find_dependency(&self, id: DependencyId) -> Option<&Dependency> {
        self.dependencies.iter().find(|edge| edge.id == id)
    }

    /// Constraints that currently gate execution and remaining projections; waived edges are omitted.
    pub fn enforced_dependencies(&self) -> impl Iterator<Item = &Dependency> {
        self.dependencies
            .iter()
            .filter(|edge| edge.waiver.is_none())
    }

    /// Find a gate by its human-readable key.
    pub fn find_decision_by_key(&self, key: &str) -> Option<&Decision> {
        self.decisions
            .values()
            .find(|decision| decision.key.0 == key)
    }
}

fn read_format_version<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version != 3 {
        return Err(serde::de::Error::custom(format!(
            "unsupported plan format {version}; expected 3. Preserve the original database and export with its original DPM release; convert that export before initializing a new store"
        )));
    }
    Ok(version)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
