//! Per-item interchange reports; they describe a candidate or export and never change state.

use dpm_model::{DependencyId, Key, WorkItemId, WorkKind, WorkStatus};
use serde::{Deserialize, Serialize};

/// One field-level observation about a mapped value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Source or DPM field the observation concerns.
    pub field: String,
    /// What was approximated, rejected or omitted, and why.
    pub detail: String,
}

impl Finding {
    pub(crate) fn new(field: &str, detail: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            detail: detail.into(),
        }
    }
}

/// One local field that the candidate changes on existing work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldChange {
    /// DPM field: `title`, `objective`, `kind`, `parent`, `priority` or `estimate`.
    pub field: String,
    /// Local value before the import.
    pub before: String,
    /// Value in the candidate.
    pub after: String,
}

/// Identity of the source project named by the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSummary {
    /// Document format; always `mspdi`.
    pub format: String,
    /// Source project GUID, when the document records one.
    pub project_guid: Option<String>,
    /// Source project name, when present.
    pub name: Option<String>,
}

/// Local work that a source task maps to in the candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkReference {
    /// Stable work identity: the task GUID, or an identity derived from project GUID and UID.
    pub id: WorkItemId,
    /// Work key; existing keys are kept, new work receives `PREFIX-UID`.
    pub key: Key,
    /// Mapped work kind.
    pub kind: WorkKind,
    /// Lifecycle in the candidate; imports never advance it.
    pub status: WorkStatus,
}

/// What the candidate does with one source task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemOutcome {
    /// New work is proposed.
    Created,
    /// Existing work with the same identity changes.
    Updated,
    /// Existing work with the same identity already matches.
    Unchanged,
    /// The task is not imported; `rejected` gives the reason.
    Skipped,
}

/// Mapping report for one source task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemReport {
    /// Source task UID, unique within the document only.
    pub uid: i64,
    /// Source task GUID, when present.
    pub guid: Option<String>,
    /// Source task name.
    pub name: String,
    /// Candidate action.
    pub outcome: ItemOutcome,
    /// Local work in the candidate; absent when skipped.
    pub work: Option<WorkReference>,
    /// Source fields carried over exactly.
    pub preserved: Vec<String>,
    /// Source fields carried over with a documented loss of precision or meaning, and defaults
    /// applied to new work where the source omits a field.
    pub approximated: Vec<Finding>,
    /// Source data present in the document that the candidate does not carry.
    pub rejected: Vec<Finding>,
    /// Fields the source omits or leaves empty, so existing work keeps its local value.
    pub kept: Vec<String>,
    /// Every local field the candidate changes on existing work; empty unless `Updated`.
    pub changes: Vec<FieldChange>,
}

/// How one source `PredecessorLink` maps to dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkOutcome {
    /// Relation and lag carried over exactly.
    Preserved,
    /// Carried over with an approximated lag.
    Approximated,
    /// Not carried over; `notes` gives the reason.
    Rejected,
}

/// Mapping report for one source `PredecessorLink`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkReport {
    /// Predecessor task UID, when the link names one.
    pub predecessor_uid: Option<i64>,
    /// Successor task UID: the task that holds the link.
    pub successor_uid: i64,
    /// Relation abbreviation, or the raw type code when unsupported.
    pub relation: Option<String>,
    /// Source lag in tenths of a minute.
    pub link_lag: i64,
    /// Source lag format code.
    pub lag_format: Option<u32>,
    /// Candidate action.
    pub outcome: LinkOutcome,
    /// Expansion, merge, approximation or rejection details.
    pub notes: Vec<String>,
    /// Candidate dependencies carrying this link.
    pub dependencies: Vec<DependencyId>,
}

/// Full import report returned with the candidate and its reviewed preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportReport {
    /// Source document identity.
    pub source: SourceSummary,
    /// Project receiving the imported work.
    pub target_project: Key,
    /// One entry per source task, in document order.
    pub items: Vec<ItemReport>,
    /// One entry per source link, in document order.
    pub links: Vec<LinkReport>,
    /// Document-level data the candidate does not carry, such as calendars and resources.
    pub rejected: Vec<Finding>,
    /// Work in the target project that the source does not name; the candidate keeps it unchanged.
    pub retained: Vec<Key>,
}

/// Export report for one work item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemExportReport {
    /// Work key.
    pub key: Key,
    /// Written task UID; unique within this file only.
    pub uid: u32,
    /// Written task GUID: the stable work identity.
    pub guid: String,
    /// Fields written exactly.
    pub preserved: Vec<String>,
    /// Fields written with a documented loss of precision.
    pub approximated: Vec<Finding>,
    /// DPM data with no MSPDI representation in the supported subset.
    pub omitted: Vec<Finding>,
}

/// Export report for one dependency touching the exported project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyExportReport {
    /// Stable dependency identity.
    pub id: DependencyId,
    /// Whether the relation is written.
    pub written: bool,
    /// DPM attributes with no MSPDI representation, or the reason it is not written.
    pub notes: Vec<String>,
}

/// Full export report returned with the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportReport {
    /// Exported project key.
    pub project: Key,
    /// Written project GUID: the stable project identity.
    pub project_guid: String,
    /// One entry per written task, in document order.
    pub items: Vec<ItemExportReport>,
    /// Dependencies with at least one endpoint in the project.
    pub dependencies: Vec<DependencyExportReport>,
    /// Workspace data outside the supported subset.
    pub omitted: Vec<Finding>,
}
