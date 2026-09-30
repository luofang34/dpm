//! Write one project's work as the supported MSPDI subset.
//!
//! Output depends only on the plan: tasks follow the outline with siblings in explicit order, UIDs
//! number that order, and GUIDs are the stable work and project identities. Without workspace
//! calendars, durations and lags are written in elapsed hours. With them, the calendars in use are
//! written too, and durations and lags on working time are written in working hours.

mod reports;

use super::calendars::export::CalendarExport;
use super::encoding::{
    ELAPSED_HOURS_FORMAT, duration_seconds, format_duration, format_guid, lag_tenths,
    priority_value, relation_code,
};
use super::report::ExportReport;
use crate::InterchangeError;
use dpm_model::{Plan, Project, WorkItem, WorkItemId, WorkKind};
use reports::{dependency_reports, item_report, project_omissions};
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
    let calendars = (plan.calendars.as_ref())
        .map(|calendars| CalendarExport::new(calendars, rows.iter().map(|r| r.work)));
    let unrepresentable = |reason| InterchangeError::Unrepresentable {
        key: project.key.0.clone(),
        reason,
    };
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Project xmlns=\"http://schemas.microsoft.com/project\">\n    <SaveVersion>14</SaveVersion>\n",
    );
    let title = escape(&project.title).map_err(unrepresentable)?;
    let project_guid = format_guid(project.id.0);
    push_line(&mut xml, 1, &format!("<Name>{title}</Name>"));
    push_line(&mut xml, 1, &format!("<GUID>{project_guid}</GUID>"));
    push_line(&mut xml, 1, &format!("<Title>{title}</Title>"));
    if let Some(calendars) = &calendars {
        let uid = calendars.project_uid();
        push_line(&mut xml, 1, &format!("<CalendarUID>{uid}</CalendarUID>"));
    }
    push_line(
        &mut xml,
        1,
        &format!(
            "<ExtendedAttributes><ExtendedAttribute><FieldID>{}</FieldID><FieldName>Text1</FieldName><Alias>{}</Alias></ExtendedAttribute></ExtendedAttributes>",
            super::metadata::FIELD_ID,
            super::metadata::ALIAS
        ),
    );
    if let Some(calendars) = &calendars {
        calendars.write(&mut xml).map_err(unrepresentable)?;
    }
    push_line(&mut xml, 1, "<Tasks>");
    let mut items = Vec::new();
    for row in &rows {
        write_task(&mut xml, plan, row, (&uids, calendars.as_ref())).map_err(|reason| {
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
            omitted: project_omissions(plan, project, &uids)
                .into_iter()
                .chain(calendars.iter().flat_map(CalendarExport::omissions))
                .collect(),
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

/// Task UIDs by work, and the calendar numbering when the workspace has calendars.
type Numbering<'a, 'c> = (
    &'a BTreeMap<WorkItemId, u32>,
    Option<&'a CalendarExport<'c>>,
);

fn write_task(
    xml: &mut String,
    plan: &Plan,
    row: &Row,
    (uids, calendars): Numbering,
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
    write_duration(xml, work, calendars);
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
    if let Some(uid) = calendars.and_then(|c| c.task_uid(work)) {
        push_line(xml, 3, &format!("<CalendarUID>{uid}</CalendarUID>"));
    }
    if !work.contract.objective.trim().is_empty() {
        push_line(
            xml,
            3,
            &format!("<Notes>{}</Notes>", escape(&work.contract.objective)?),
        );
    }
    let metadata = escape(&super::metadata::encode(work)?)?;
    push_line(
        xml,
        3,
        &format!(
            "<ExtendedAttribute><FieldID>{}</FieldID><Value>{metadata}</Value></ExtendedAttribute>",
            super::metadata::FIELD_ID
        ),
    );
    write_links(xml, plan, work.id, (uids, calendars));
    push_line(xml, 2, "</Task>");
    Ok(())
}

/// Duration of a task or milestone in working hours of its calendar, or in elapsed hours without
/// calendars or on `always`.
fn write_duration(xml: &mut String, work: &WorkItem, calendars: Option<&CalendarExport>) {
    let seconds = match work.kind {
        WorkKind::Task => duration_seconds(work.schedule.estimate),
        WorkKind::Milestone => 0,
        WorkKind::WorkPackage => return,
    };
    let format = calendars.map_or(ELAPSED_HOURS_FORMAT, |c| c.duration_format(work));
    push_line(
        xml,
        3,
        &format!("<Duration>{}</Duration>", format_duration(seconds)),
    );
    push_line(
        xml,
        3,
        &format!("<DurationFormat>{format}</DurationFormat>"),
    );
}

/// Links into one successor from exported predecessors, ordered by predecessor UID and type.
fn write_links(xml: &mut String, plan: &Plan, successor: WorkItemId, (uids, calendars): Numbering) {
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
            (
                "LagFormat",
                calendars
                    .map_or(ELAPSED_HOURS_FORMAT, |_| CalendarExport::lag_format(edge))
                    .to_string(),
            ),
        ] {
            push_line(xml, 4, &format!("<{name}>{value}</{name}>"));
        }
        push_line(xml, 3, "</PredecessorLink>");
    }
}

pub(super) fn push_line(xml: &mut String, depth: usize, line: &str) {
    xml.push_str(&" ".repeat(depth * 4));
    xml.push_str(line);
    xml.push('\n');
}

/// Escape text for element content; a carriage return is written as a reference so parsers do
/// not normalize it away. Characters XML 1.0 cannot carry are rejected.
pub(super) fn escape(text: &str) -> Result<String, String> {
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
