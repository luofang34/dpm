use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Temporal lower bound relating two activity endpoints.
pub enum DependencyKind {
    /// Successor start is bounded by predecessor finish plus lag.
    FinishStart,
    /// Successor start is bounded by predecessor start plus lag.
    StartStart,
    /// Successor finish is bounded by predecessor finish plus lag.
    FinishFinish,
    /// Successor finish is bounded by predecessor start plus lag.
    StartFinish,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Temporal constraint between two work items.
pub struct Dependency {
    /// Activity providing the constrained start or finish.
    pub predecessor: WorkItemId,
    /// Activity whose start or finish has the lower bound.
    pub successor: WorkItemId,
    /// Domain category of this value.
    pub kind: DependencyKind,
    /// Positive values add delay; negative values are lead time.
    pub lag_hours: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Authoritative graph and revision; scheduling and completion views are derived.
pub struct Plan {
    /// Portable domain format version; unsupported formats are rejected.
    pub format_version: u32,
    /// Resources shared across code and non-code work.
    pub resources: BTreeMap<ResourceId, Resource>,
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
}

impl Plan {
    /// Create an empty namespace at revision zero.
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            format_version: 2,
            resources: BTreeMap::new(),
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

    /// Find a gate by its human-readable key.
    pub fn find_decision_by_key(&self, key: &str) -> Option<&Decision> {
        self.decisions
            .values()
            .find(|decision| decision.key.0 == key)
    }
}
