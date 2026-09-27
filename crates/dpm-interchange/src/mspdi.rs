//! Microsoft Project XML (MSPDI) subset.
//!
//! Supported: tasks, summary tasks, milestones, outline hierarchy, names, notes, priority,
//! durations as single-point estimates, and FS/SS/FF/SF predecessor links with hour-based lags.
//! Everything else found in a document is reported per item and not imported.

mod conditional;
mod encoding;
mod export;
mod items;
mod links;
mod report;
mod source;
mod unsupported;

use crate::InterchangeError;
use dpm_model::{Plan, ProjectId};
use serde::{Deserialize, Serialize};

pub use export::{ExportResult, export_mspdi};
pub use report::{
    DependencyExportReport, ExportReport, FieldChange, Finding, ImportReport, ItemExportReport,
    ItemOutcome, ItemReport, LinkOutcome, LinkReport, RemovedDependency, SourceSummary,
    WorkReference,
};

/// Where imported work goes and how new work is keyed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportOptions {
    /// Existing project that receives the imported work.
    pub project_key: String,
    /// Prefix for keys of new work (`PREFIX-UID`); defaults to the project key.
    #[serde(default)]
    pub key_prefix: Option<String>,
}

/// Candidate plan and its report; the candidate is reviewed and applied through the plan-change
/// path, never written directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportResult {
    /// Full candidate snapshot at the current revision.
    pub candidate: Plan,
    /// Per-item preserved, approximated and rejected source data.
    pub report: ImportReport,
}

/// Build a candidate plan that maps an MSPDI document onto `current` without changing it.
///
/// Work identity is the task GUID (or, without one, an identity derived from the project GUID
/// and task UID), so importing the same document again updates the same work. Lifecycle,
/// ownership, progress, evidence and acceptance of existing work are never touched, and new
/// tasks are `Proposed` because MSPDI carries no acceptance criteria.
pub fn import_mspdi(
    current: &Plan,
    xml: &str,
    options: &ImportOptions,
) -> Result<ImportResult, InterchangeError> {
    let source = source::parse(xml)?;
    let project = project_id(current, &options.project_key)?;
    let prefix = options
        .key_prefix
        .clone()
        .unwrap_or_else(|| options.project_key.clone());
    let outline = items::map(current, &source, project, &prefix)?;
    let mut candidate = current.clone();
    for mapped in outline.mapped() {
        candidate.work_items.insert(mapped.id, mapped.clone());
    }
    let links = links::map(current, &source, &outline);
    candidate.dependencies = links.dependencies;
    let mut retained: Vec<_> = current
        .work_items
        .values()
        .filter(|w| w.project == project && !outline.contains(w.id))
        .map(|w| w.key.clone())
        .collect();
    retained.sort();
    Ok(ImportResult {
        candidate,
        report: ImportReport {
            source: SourceSummary {
                format: "mspdi".into(),
                project_guid: source.guid.map(encoding::format_guid),
                name: source.name.clone(),
            },
            target_project: dpm_model::Key::new(options.project_key.clone()),
            items: outline.reports,
            links: links.reports,
            removed_dependencies: links.removed,
            rejected: source.rejected,
            retained,
        },
    })
}

fn project_id(plan: &Plan, key: &str) -> Result<ProjectId, InterchangeError> {
    plan.projects
        .values()
        .find(|p| p.key.0 == key)
        .map(|p| p.id)
        .ok_or_else(|| InterchangeError::UnknownProject { key: key.into() })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
