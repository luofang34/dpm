//! Export reports: what each written item and dependency preserves, approximates or omits.

use super::Row;
use crate::mspdi::encoding::{duration_seconds, format_guid};
use crate::mspdi::report::{DependencyExportReport, Finding, ItemExportReport};
use dpm_model::{Plan, Project, WorkItem, WorkItemId, WorkKind, WorkStatus};
use std::collections::BTreeMap;

pub(super) fn item_report(
    plan: &Plan,
    applicability: &BTreeMap<WorkItemId, dpm_model::Applicability>,
    row: &Row,
) -> ItemExportReport {
    let work = row.work;
    let mut preserved = vec![
        "identity".to_string(),
        "key".into(),
        "dpm_metadata".into(),
        "title".into(),
        "kind".into(),
        "outline".into(),
        "priority".into(),
    ];
    let mut approximated = Vec::new();
    if !work.contract.objective.trim().is_empty() {
        preserved.push("objective".into());
    }
    if plan.calendars.is_some() && work.kind != WorkKind::WorkPackage {
        preserved.push("calendar".into());
    }
    if work.kind == WorkKind::Task {
        let exact = work.schedule.estimate.is_none_or(|e| {
            e.optimistic_hours == e.pessimistic_hours
                && duration_seconds(Some(e)) as f64 == e.likely_hours * 3600.0
        });
        match work.schedule.estimate {
            Some(e) if !exact => approximated.push(Finding::new(
                "estimate",
                format!(
                    "three-point estimate {}/{}/{} h retained in DPM.Metadata.v1; display duration is its PERT expectation rounded to whole seconds",
                    e.optimistic_hours, e.likely_hours, e.pessimistic_hours
                ),
            )),
            _ => preserved.push("estimate".into()),
        }
    }
    ItemExportReport {
        key: work.key.clone(),
        uid: row.uid,
        guid: format_guid(work.id.0),
        preserved,
        approximated,
        omitted: omissions(work)
            .into_iter()
            .chain(crate::mspdi::conditional::findings(
                plan,
                applicability,
                work,
            ))
            .collect(),
    }
}

fn omissions(work: &WorkItem) -> Vec<Finding> {
    let mut omitted = Vec::new();
    let mut note = |present: bool, field: &str, detail: String| {
        if present {
            omitted.push(Finding::new(field, detail));
        }
    };
    let initial = if work.kind == WorkKind::Task {
        WorkStatus::Proposed
    } else {
        WorkStatus::Planned
    };
    note(
        work.execution.status != initial,
        "status",
        format!(
            "lifecycle {:?} is not written; MSPDI progress fields stay empty",
            work.execution.status
        ),
    );
    note(
        !work.contract.acceptance.is_empty(),
        "acceptance",
        format!("{} acceptance criteria", work.contract.acceptance.len()),
    );
    note(
        work.contract.instructions.is_some(),
        "instructions",
        "execution instructions".into(),
    );
    note(
        work.execution.owner.is_some(),
        "owner",
        "claim owner".into(),
    );
    note(
        work.execution.reported_progress_percent != 0,
        "reported_progress_percent",
        format!("{}% owner report", work.execution.reported_progress_percent),
    );
    note(
        !work.contract.capabilities.is_empty(),
        "capabilities",
        "required capabilities".into(),
    );
    note(
        !work.contract.requirement_ids.is_empty(),
        "requirement_ids",
        "requirement links".into(),
    );
    note(
        !work.execution.artifact_ids.is_empty(),
        "artifact_ids",
        "attached evidence".into(),
    );
    note(
        !work.contract.assets.is_empty(),
        "assets",
        "workspace asset requirements".into(),
    );
    note(
        work.execution.last_rejection.is_some(),
        "last_rejection",
        "latest review rejection".into(),
    );
    note(
        work.execution.block_reason.is_some(),
        "block_reason",
        "blocker".into(),
    );
    omitted
}

pub(super) fn dependency_reports(
    plan: &Plan,
    applicability: &BTreeMap<WorkItemId, dpm_model::Applicability>,
    uids: &BTreeMap<WorkItemId, u32>,
) -> Vec<DependencyExportReport> {
    let mut reports: Vec<_> = plan
        .dependencies
        .iter()
        .filter(|d| uids.contains_key(&d.predecessor) || uids.contains_key(&d.successor))
        .map(|edge| {
            let written =
                uids.contains_key(&edge.predecessor) && uids.contains_key(&edge.successor);
            let mut notes = Vec::new();
            if !written {
                notes.push("an endpoint belongs to another project; not written".into());
            }
            if edge.policy == dpm_model::DependencyPolicy::Soft {
                notes.push("Soft policy is not represented; MSPDI links always apply".into());
            }
            if edge.start_basis == dpm_model::StartBasis::Provisional {
                notes.push(
                    "provisional start basis is not represented; MSPDI links wait for the finish"
                        .into(),
                );
            }
            if edge.rationale.is_some() {
                notes.push("rationale is not represented".into());
            }
            if edge.waiver.is_some() {
                notes.push("waiver is not represented; the link is written as enforced".into());
            }
            notes.extend(crate::mspdi::conditional::edge_note(applicability, edge));
            if (edge.lag_hours * 600.0).fract() != 0.0 {
                notes.push("lag rounded to a tenth of a minute".into());
            }
            DependencyExportReport {
                id: edge.id,
                written,
                notes,
            }
        })
        .collect();
    reports.sort_by_key(|r| r.id);
    reports
}

pub(super) fn project_omissions(
    plan: &Plan,
    project: &Project,
    uids: &BTreeMap<WorkItemId, u32>,
) -> Vec<Finding> {
    let mut omitted = Vec::new();
    let mut count = |field: &str, found: usize, what: &str| {
        if found > 0 {
            omitted.push(Finding::new(
                field,
                format!("{found} {what} not represented in MSPDI"),
            ));
        }
    };
    let in_project = |id| id == project.id;
    count(
        "requirements",
        plan.requirements
            .values()
            .filter(|r| in_project(r.project))
            .count(),
        "project requirement(s)",
    );
    count(
        "decisions",
        plan.decisions
            .values()
            .filter(|d| in_project(d.project))
            .count(),
        "project decision(s)",
    );
    count(
        "risks",
        plan.risks
            .values()
            .filter(|r| in_project(r.project))
            .count(),
        "project risk(s)",
    );
    count(
        "links",
        plan.links
            .iter()
            .filter(|l| uids.contains_key(&l.source) || uids.contains_key(&l.target))
            .count(),
        "work link(s)",
    );
    count(
        "external_references",
        plan.external_references
            .values()
            .filter(|r| r.links.iter().any(|l| uids.contains_key(&l.work)))
            .count(),
        "external reference(s)",
    );
    count(
        "child_projects",
        plan.projects
            .values()
            .filter(|p| p.parent == Some(project.id))
            .count(),
        "child project(s), exported separately,",
    );
    omitted
}
