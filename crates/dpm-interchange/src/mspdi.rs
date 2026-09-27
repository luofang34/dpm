//! Microsoft Project XML (MSPDI) subset.
//!
//! Supported: tasks, summary tasks, milestones, outline hierarchy, names, notes, priority,
//! durations and carried three-point estimates, and FS/SS/FF/SF predecessor links with hour-based lags.
//! Everything else found in a document is reported per item and not imported.

mod conditional;
mod encoding;
mod export;
mod items;
mod links;
mod metadata;
mod refusal;
mod report;
mod source;
mod unsupported;

use crate::InterchangeError;
use dpm_model::{Plan, ProjectId};
use serde::{Deserialize, Serialize};

pub use export::{ExportResult, export_mspdi};
pub use refusal::{LinkEndpoint, SourceChange, SourceLinkChange};
pub use report::{
    DependencyChange, DependencyExportReport, ExportReport, FieldChange, Finding, ImportReport,
    ItemExportReport, ItemOutcome, ItemReport, LinkOutcome, LinkReport, RemovedDependency,
    SourceSummary, WorkReference,
};

/// Stated in every import report so no reader mistakes the candidate for a dated schedule.
const IMPORT_SCOPE: &str = concat!(
    "Imports plan structure (outline, kinds, names, notes, priority, durations, relations and ",
    "lags), not calendar dates. Working-time durations and lags become continuous elapsed hours; ",
    "calendars (including resource calendars), resources, assignments and date constraints are ",
    "reported as not imported; source start and finish dates are ignored because DPM derives ",
    "dates from the graph."
);

/// Where imported work goes and how new work is keyed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportOptions {
    /// Existing project that receives the imported work.
    pub project_key: String,
    /// Prefix for keys of new work (`PREFIX-UID`); defaults to the project key. A document without
    /// any GUID requires it: it names the source, and GUID-less identities derive from it.
    #[serde(default)]
    pub key_prefix: Option<String>,
    /// How a task without a GUID may match existing work in the target project; `None` never
    /// matches, so such a task always maps to its derived identity.
    #[serde(default)]
    pub match_existing_by: Option<ExistingMatch>,
    /// Keep the local priority of existing work whatever the source says, and report the source
    /// value instead. OmniPlan rescales priorities by the highest one in the document, so an
    /// unedited round trip without a P0 raises every band.
    #[serde(default)]
    pub keep_existing_priority: bool,
}

/// Explicit rule for mapping GUID-less source tasks onto existing work, for files that went
/// through a tool which drops GUIDs (OmniPlan) and come back to the plan they were exported from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExistingMatch {
    /// A task matches the one work item in the target project whose titles from the project root
    /// down equal the task's outline path of names; a path shared by several local items, or by
    /// several GUID-less tasks that could match, refuses the import.
    TitlePath,
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
/// Namespaced DPM metadata preserves identity, keys and estimates. Otherwise identity is the task GUID; without one, an identity derived from the project GUID and task
/// UID; without either GUID, one derived from the target project, the explicit key prefix and the
/// task UID. Importing the same document again (with the same prefix) updates the same work. Lifecycle,
/// ownership, progress, evidence and acceptance of existing work are never touched, and new
/// tasks are `Proposed` because MSPDI carries no acceptance criteria.
pub fn import_mspdi(
    current: &Plan,
    xml: &str,
    options: &ImportOptions,
) -> Result<ImportResult, InterchangeError> {
    let source = source::parse(xml)?;
    let project = project_id(current, &options.project_key)?;
    require_source_scope(&source, options)?;
    let prefix = options
        .key_prefix
        .clone()
        .unwrap_or_else(|| options.project_key.clone());
    let resolver = items::Resolver::new(
        current,
        &source,
        (project, &options.project_key),
        &prefix,
        options.match_existing_by,
    )?;
    let fields = items::FieldPolicy {
        keep_existing_priority: options.keep_existing_priority,
    };
    let outline = items::map(current, &source, &resolver, (project, &prefix), fields)?;
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
                scope: IMPORT_SCOPE.into(),
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

/// Without any GUID, the key prefix is the only name of the source, so it must be stated rather
/// than defaulted: a default would make every GUID-less file imported into the project the same
/// source and silently merge unrelated tasks that share a UID.
fn require_source_scope(
    source: &source::SourceProject,
    options: &ImportOptions,
) -> Result<(), InterchangeError> {
    if source.guid.is_some() || options.key_prefix.is_some() {
        return Ok(());
    }
    let anonymous: Vec<i64> = source
        .tasks
        .iter()
        .filter(|t| {
            t.guid.is_none()
                && t.metadata.is_none()
                && t.exclusion.is_none()
                && !t.is_project_summary()
        })
        .map(|t| t.uid)
        .collect();
    match anonymous.first() {
        None => Ok(()),
        Some(first) => Err(InterchangeError::SourceScopeRequired {
            count: anonymous.len(),
            first_uid: *first,
        }),
    }
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
