use crate::{ActorId, ArtifactId, DecisionId, Key, ProjectId, RequirementId, RiskId, WorkItemId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Traceable motivation for one or more work items.
pub struct Requirement {
    /// Stable entity identity; it must match its containing map key.
    pub id: RequirementId,
    /// Human-readable key, unique in this entity category.
    pub key: Key,
    /// Project that owns this entity.
    pub project: ProjectId,
    /// Non-empty human-readable title.
    pub title: String,
    /// Requirement that the implementation must satisfy.
    pub statement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Category of externally stored work evidence.
pub enum ArtifactKind {
    /// A Git commit.
    GitCommit,
    /// A code review request.
    PullRequest,
    /// An external file.
    File,
    /// A build output.
    Build,
    /// Acceptance or test evidence.
    TestResult,
    /// A component specification.
    Datasheet,
    /// A supplier quotation.
    Quote,
    /// A procurement order.
    PurchaseOrder,
    /// A design file.
    Cad,
    /// A photograph.
    Photo,
    /// Evidence outside the standard categories.
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Reference to evidence with its creating principal and timestamp.
pub struct Artifact {
    /// Stable entity identity; it must match its containing map key.
    pub id: ArtifactId,
    /// Domain category of this value.
    pub kind: ArtifactKind,
    /// Non-empty locator of the evidence.
    pub uri: String,
    /// Human-readable evidence description.
    pub label: String,
    /// Artifact-specific attributes, such as a Git commit hash.
    pub metadata: BTreeMap<String, String>,
    /// Principal responsible for this evidence.
    pub created_by: ActorId,
    /// Timestamp at which the evidence was recorded.
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Lifecycle of a decision that can gate execution.
pub enum DecisionStatus {
    /// Unresolved gate.
    Open,
    /// Resolved gate with an outcome.
    Decided,
    /// Gate no longer applicable.
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Question whose resolution can unblock work.
pub struct Decision {
    /// Stable entity identity; it must match its containing map key.
    pub id: DecisionId,
    /// Human-readable key, unique in this entity category.
    pub key: Key,
    /// Project that owns this entity.
    pub project: ProjectId,
    /// Question requiring a decision.
    pub question: String,
    /// Whether the question still gates execution.
    pub status: DecisionStatus,
    /// Non-empty resolution for a decided gate.
    pub outcome: Option<String>,
    /// Work items gated by this decision.
    pub blocks: BTreeSet<WorkItemId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Qualitative consequence of a risk.
pub enum RiskImpact {
    /// Small consequence.
    Low,
    /// Moderate consequence.
    Medium,
    /// Large consequence.
    High,
    /// Project-threatening consequence.
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Identified uncertainty with probability and related work.
pub struct Risk {
    /// Stable entity identity; it must match its containing map key.
    pub id: RiskId,
    /// Human-readable key, unique in this entity category.
    pub key: Key,
    /// Project that owns this entity.
    pub project: ProjectId,
    /// Risk event or condition.
    pub description: String,
    /// Probability in [0, 1].
    pub probability: f64,
    /// Qualitative consequence if the risk occurs.
    pub impact: RiskImpact,
    /// Work affected by the risk.
    pub related_work: BTreeSet<WorkItemId>,
    /// Optional response to reduce the risk.
    pub mitigation: Option<String>,
}
