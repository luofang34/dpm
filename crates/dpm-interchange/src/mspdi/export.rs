//! Write one project's work as the supported MSPDI subset.
//!
//! Output depends only on the plan: tasks follow the outline with siblings in explicit order, UIDs
//! number that order, and GUIDs are the stable work and project identities. Durations and lags
//! are written in elapsed hours because DPM schedules elapsed hours.

use super::encoding::{
    ELAPSED_HOURS_FORMAT, duration_seconds, format_duration, format_guid, lag_tenths,
    priority_value, relation_code,
};
use super::report::{DependencyExportReport, ExportReport, Finding, ItemExportReport};
use crate::InterchangeError;
use dpm_model::{Plan, Project, WorkItem, WorkItemId, WorkKind, WorkStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Written document and its report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportResult {
    /// Complete MSPDI document.
    pub xml: String,
    /// Per-item preserved, approximated and omitted data.
    pub report: ExportReport,
}

struct Row<'a> {
    work: &'a WorkItem,
    uid: u32,
    outline_number: String,
    level: usize,
}

/// Export the work of one project (not its child projects) as MSPDI.
pub fn export_mspdi(plan: &Plan, project_key: &str) -> Result<ExportResult, InterchangeError> {
    let project = plan
        .projects
        .values()
        .find(|p| p.key.0 == project_key)
        .ok_or_else(|| InterchangeError::UnknownProject {
            key: project_key.into(),
        })?;
    let rows = outline(plan, project);
    let applicability = plan.applicability();
    let uids: BTreeMap<WorkItemId, u32> = rows.iter().map(|r| (r.work.id, r.uid)).collect();
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Project xmlns=\"http://schemas.microsoft.com/project\">\n    <SaveVersion>14</SaveVersion>\n",
    );
    let title = escape(&project.title).map_err(|reason| InterchangeError::Unrepresentable {
        key: project.key.0.clone(),
        reason,
    })?;
    let project_guid = format_guid(project.id.0);
    push_line(&mut xml, 1, &format!("<Name>{title}</Name>"));
    push_line(&mut xml, 1, &format!("<GUID>{project_guid}</GUID>"));
    push_line(&mut xml, 1, &format!("<Title>{title}</Title>"));
    push_line(&mut xml, 1, "<Tasks>");
    let mut items = Vec::new();
    for row in &rows {
        write_task(&mut xml, plan, row, &uids).map_err(|reason| {
            InterchangeError::Unrepresentable {
                key: row.work.key.0.clone(),
                reason,
            }
        })?;
        items.push(item_report(plan, &applicability, row));
    }
    push_line(&mut xml, 1, "</Tasks>");
    xml.push_str("</Project>\n");
    Ok(ExportResult {
        xml,
        report: ExportReport {
            project: project.key.clone(),
            project_guid,
            items,
            dependencies: dependency_reports(plan, &applicability, &uids),
            omitted: project_omissions(plan, project, &uids),
        },
    })
}

fn outline<'a>(plan: &'a Plan, project: &Project) -> Vec<Row<'a>> {
    let mut children: BTreeMap<Option<WorkItemId>, Vec<&WorkItem>> = BTreeMap::new();
    for work in plan.work_items.values().filter(|w| w.project == project.id) {
        children.entry(work.parent).or_default().push(work);
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|a, b| a.order.cmp(&b.order).then(a.id.cmp(&b.id)));
    }
    let mut rows = Vec::new();
    let roots = children.get(&None).cloned().unwrap_or_default();
    let mut pending: Vec<(&WorkItem, String)> = roots
        .iter()
        .enumerate()
        .rev()
        .map(|(i, w)| (*w, (i + 1).to_string()))
        .collect();
    while let Some((work, number)) = pending.pop() {
        let nested = children.get(&Some(work.id)).cloned().unwrap_or_default();
        pending.extend(
            nested
                .iter()
                .enumerate()
                .rev()
                .map(|(i, child)| (*child, format!("{number}.{}", i + 1))),
        );
        rows.push(Row {
            work,
            uid: u32::try_from(rows.len() + 1).unwrap_or(u32::MAX),
            level: number.split('.').count(),
            outline_number: number,
        });
    }
    rows
}

fn write_task(
    xml: &mut String,
    plan: &Plan,
    row: &Row,
    uids: &BTreeMap<WorkItemId, u32>,
) -> Result<(), String> {
    let work = row.work;
    push_line(xml, 2, "<Task>");
    for (name, value) in [
        ("UID", row.uid.to_string()),
        ("GUID", format_guid(work.id.0)),
        ("ID", row.uid.to_string()),
        ("Name", escape(&work.title)?),
        ("OutlineNumber", row.outline_number.clone()),
        ("OutlineLevel", row.level.to_string()),
        (
            "Priority",
            priority_value(work.schedule.priority).to_string(),
        ),
    ] {
        push_line(xml, 3, &format!("<{name}>{value}</{name}>"));
    }
    if work.kind != WorkKind::WorkPackage {
        let seconds = match work.kind {
            WorkKind::Task => duration_seconds(work.schedule.estimate),
            WorkKind::Milestone | WorkKind::WorkPackage => 0,
        };
        push_line(
            xml,
            3,
            &format!("<Duration>{}</Duration>", format_duration(seconds)),
        );
        push_line(
            xml,
            3,
            &format!("<DurationFormat>{ELAPSED_HOURS_FORMAT}</DurationFormat>"),
        );
    }
    let flag = |set: bool| if set { 1 } else { 0 };
    push_line(
        xml,
        3,
        &format!(
            "<Milestone>{}</Milestone>",
            flag(work.kind == WorkKind::Milestone)
        ),
    );
    push_line(
        xml,
        3,
        &format!(
            "<Summary>{}</Summary>",
            flag(work.kind == WorkKind::WorkPackage)
        ),
    );
    if !work.contract.objective.trim().is_empty() {
        push_line(
            xml,
            3,
            &format!("<Notes>{}</Notes>", escape(&work.contract.objective)?),
        );
    }
    write_links(xml, plan, work.id, uids);
    push_line(xml, 2, "</Task>");
    Ok(())
}

/// Links into one successor from exported predecessors, ordered by predecessor UID and type.
fn write_links(
    xml: &mut String,
    plan: &Plan,
    successor: WorkItemId,
    uids: &BTreeMap<WorkItemId, u32>,
) {
    let mut links: Vec<_> = plan
        .dependencies
        .iter()
        .filter(|d| d.successor == successor)
        .filter_map(|d| Some((*uids.get(&d.predecessor)?, relation_code(d.kind), d)))
        .collect();
    links.sort_by_key(|(uid, code, _)| (*uid, *code));
    for (predecessor, code, edge) in links {
        push_line(xml, 3, "<PredecessorLink>");
        for (name, value) in [
            ("PredecessorUID", predecessor.to_string()),
            ("Type", code.to_string()),
            ("CrossProject", "0".into()),
            ("LinkLag", lag_tenths(edge.lag_hours).to_string()),
            ("LagFormat", ELAPSED_HOURS_FORMAT.to_string()),
        ] {
            push_line(xml, 4, &format!("<{name}>{value}</{name}>"));
        }
        push_line(xml, 3, "</PredecessorLink>");
    }
}

fn item_report(
    plan: &Plan,
    applicability: &BTreeMap<WorkItemId, dpm_model::Applicability>,
    row: &Row,
) -> ItemExportReport {
    let work = row.work;
    let mut preserved = vec![
        "identity".to_string(),
        "title".into(),
        "kind".into(),
        "outline".into(),
        "priority".into(),
    ];
    let mut approximated = Vec::new();
    if !work.contract.objective.trim().is_empty() {
        preserved.push("objective".into());
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
                    "three-point estimate {}/{}/{} h written as its PERT expectation, rounded to whole seconds",
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
            .chain(super::conditional::findings(plan, applicability, work))
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

fn dependency_reports(
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
            notes.extend(super::conditional::edge_note(applicability, edge));
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

fn project_omissions(
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

fn push_line(xml: &mut String, depth: usize, line: &str) {
    xml.push_str(&" ".repeat(depth * 4));
    xml.push_str(line);
    xml.push('\n');
}

/// Escape text for element content; a carriage return is written as a reference so parsers do
/// not normalize it away. Characters XML 1.0 cannot carry are rejected.
fn escape(text: &str) -> Result<String, String> {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\r' => escaped.push_str("&#13;"),
            '\t' | '\n' => escaped.push(character),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {
                return Err(format!(
                    "text contains U+{:04X}, which XML 1.0 cannot carry",
                    c as u32
                ));
            }
            c => escaped.push(c),
        }
    }
    Ok(escaped)
}
