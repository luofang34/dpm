//! Read the supported MSPDI elements and record every unsupported one that is present.

use super::report::Finding;
use crate::InterchangeError;
use roxmltree::{Document, Node};
use std::collections::BTreeMap;
use uuid::Uuid;

pub(crate) const NAMESPACE: &str = "http://schemas.microsoft.com/project";
/// `ResourceUID` of an assignment that names no resource.
const UNASSIGNED_RESOURCE: i64 = -65535;

/// Supported content of one MSPDI document.
#[derive(Debug)]
pub(crate) struct SourceProject {
    pub(crate) guid: Option<Uuid>,
    pub(crate) name: Option<String>,
    pub(crate) tasks: Vec<SourceTask>,
    /// Document-level data outside the subset.
    pub(crate) rejected: Vec<Finding>,
}

/// Supported fields of one task element, plus unsupported fields found on it.
#[derive(Debug)]
pub(crate) struct SourceTask {
    pub(crate) metadata: Option<super::metadata::Metadata>,
    pub(crate) position: usize,
    pub(crate) uid: i64,
    pub(crate) guid: Option<Uuid>,
    pub(crate) name: String,
    pub(crate) outline_level: u32,
    pub(crate) duration: Option<String>,
    pub(crate) duration_format: Option<u32>,
    /// `None` when the element is absent, so re-imports can keep the local kind.
    pub(crate) milestone: Option<bool>,
    pub(crate) summary: bool,
    pub(crate) priority: Option<i64>,
    pub(crate) notes: Option<String>,
    /// Why the task is not imported at all, when it is excluded.
    pub(crate) exclusion: Option<String>,
    pub(crate) rejected: Vec<Finding>,
    pub(crate) links: Vec<SourceLink>,
}

/// One `PredecessorLink` element.
#[derive(Debug)]
pub(crate) struct SourceLink {
    pub(crate) predecessor_uid: Option<i64>,
    pub(crate) type_code: Option<i64>,
    pub(crate) cross_project: bool,
    pub(crate) link_lag: i64,
    pub(crate) lag_format: Option<u32>,
}

impl SourceTask {
    /// The UID 0 / outline level 0 task summarizes the whole project, which maps to the target
    /// project rather than to work.
    pub(crate) fn is_project_summary(&self) -> bool {
        self.uid == 0 || self.outline_level == 0
    }
}

pub(crate) fn parse(xml: &str) -> Result<SourceProject, InterchangeError> {
    let document = Document::parse(xml).map_err(|source| InterchangeError::Xml { source })?;
    let root = document.root_element();
    if root.tag_name().name() != "Project" || root.tag_name().namespace() != Some(NAMESPACE) {
        return Err(InterchangeError::NotMspdi {
            found: format!(
                "{{{}}}{}",
                root.tag_name().namespace().unwrap_or_default(),
                root.tag_name().name()
            ),
        });
    }
    let metadata_field = super::metadata::field(root)?;
    let resources = resource_names(root);
    let assignments = assignments(root, &resources);
    let mut tasks = Vec::new();
    for (index, node) in children(root, "Tasks")
        .flat_map(|tasks| children(tasks, "Task"))
        .enumerate()
    {
        let mut task = parse_task(node, index + 1, metadata_field.as_deref())?;
        if let Some(found) = assignments.get(&task.uid) {
            task.rejected.extend(found.iter().cloned());
        }
        tasks.push(task);
    }
    Ok(SourceProject {
        guid: optional_guid(text(root, "GUID"))
            .map_err(|reason| InterchangeError::MalformedProject { reason })?,
        // OmniPlan names the project only in <Title>.
        name: text(root, "Name")
            .or_else(|| text(root, "Title"))
            .map(str::to_owned),
        tasks,
        rejected: document_findings(root, resources.len()),
    })
}

fn document_findings(root: Node, resources: usize) -> Vec<Finding> {
    let mut findings = Vec::new();
    let calendars = children(root, "Calendars")
        .flat_map(|c| children(c, "Calendar"))
        .count();
    if calendars > 0 {
        findings.push(Finding::new(
            "calendars",
            format!("{calendars} calendar(s) not imported; DPM schedules elapsed hours"),
        ));
    }
    if resources > 0 {
        findings.push(Finding::new(
            "resources",
            format!("{resources} resource(s) not imported; DPM workspace assets are repositories and tools"),
        ));
    }
    let definitions = children(root, "ExtendedAttributes")
        .flat_map(|c| children(c, "ExtendedAttribute"))
        .filter(|n| text(*n, "Alias") != Some(super::metadata::ALIAS))
        .count();
    if definitions > 0 {
        findings.push(Finding::new(
            "custom_fields",
            format!("{definitions} custom field definition(s) not imported"),
        ));
    }
    findings
}

fn resource_names(root: Node) -> BTreeMap<i64, String> {
    children(root, "Resources")
        .flat_map(|r| children(r, "Resource"))
        .filter(|r| text(*r, "IsNull") != Some("1"))
        .filter_map(|r| {
            let uid = text(r, "UID")?.trim().parse::<i64>().ok()?;
            (uid != 0).then(|| (uid, text(r, "Name").unwrap_or_default().to_owned()))
        })
        .collect()
}

fn assignments(root: Node, resources: &BTreeMap<i64, String>) -> BTreeMap<i64, Vec<Finding>> {
    let mut found: BTreeMap<i64, Vec<Finding>> = BTreeMap::new();
    for assignment in children(root, "Assignments").flat_map(|a| children(a, "Assignment")) {
        let uid = |name| text(assignment, name).and_then(|v| v.trim().parse::<i64>().ok());
        let (Some(task), Some(resource)) = (uid("TaskUID"), uid("ResourceUID")) else {
            continue;
        };
        if resource == UNASSIGNED_RESOURCE {
            continue;
        }
        let name = resources.get(&resource).map_or("", String::as_str);
        found.entry(task).or_default().push(Finding::new(
            "assignments",
            format!("assignment of resource UID {resource} {name:?} not imported"),
        ));
    }
    found
}

fn parse_task(
    node: Node,
    position: usize,
    metadata_field: Option<&str>,
) -> Result<SourceTask, InterchangeError> {
    let malformed = |reason: String| InterchangeError::MalformedTask { position, reason };
    let integer = |name: &'static str| -> Result<Option<i64>, InterchangeError> {
        text(node, name)
            .map(|value| {
                value
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| malformed(format!("{name} {value:?} is not an integer")))
            })
            .transpose()
    };
    let uid = integer("UID")?.ok_or_else(|| malformed("UID is missing".into()))?;
    let outline_level = integer("OutlineLevel")?
        .ok_or_else(|| malformed(format!("task UID {uid} has no OutlineLevel")))?;
    let outline_level = u32::try_from(outline_level)
        .map_err(|_| malformed(format!("task UID {uid} has a negative OutlineLevel")))?;
    let format = |name: &'static str| -> Result<Option<u32>, InterchangeError> {
        integer(name)?
            .map(|v| u32::try_from(v).map_err(|_| malformed(format!("{name} {v} is negative"))))
            .transpose()
    };
    let mut links = Vec::new();
    for link in children(node, "PredecessorLink") {
        links.push(parse_link(link).map_err(malformed)?);
    }
    Ok(SourceTask {
        metadata: super::metadata::parse(node, metadata_field, position)?,
        position,
        uid,
        guid: optional_guid(text(node, "GUID")).map_err(malformed)?,
        name: text(node, "Name").unwrap_or_default().to_owned(),
        outline_level,
        duration: text(node, "Duration").map(str::to_owned),
        duration_format: format("DurationFormat")?,
        milestone: text(node, "Milestone").map(|v| matches!(v.trim(), "1" | "true")),
        summary: flag(node, "Summary"),
        priority: integer("Priority")?,
        // Trailing line breaks are not content; OmniPlan ends every note with one.
        notes: text(node, "Notes")
            .map(str::trim_end)
            .filter(|n| !n.trim().is_empty())
            .map(str::to_owned),
        exclusion: exclusion(node),
        rejected: super::unsupported::task_findings(node, metadata_field),
        links,
    })
}

fn parse_link(link: Node) -> Result<SourceLink, String> {
    let integer = |name: &'static str| -> Result<Option<i64>, String> {
        text(link, name)
            .map(|v| {
                v.trim()
                    .parse::<i64>()
                    .map_err(|_| format!("PredecessorLink {name} {v:?} is not an integer"))
            })
            .transpose()
    };
    let lag_format = integer("LagFormat")?
        .map(|v| u32::try_from(v).map_err(|_| format!("LagFormat {v} is negative")))
        .transpose()?;
    Ok(SourceLink {
        predecessor_uid: integer("PredecessorUID")?,
        type_code: integer("Type")?,
        cross_project: flag(link, "CrossProject"),
        link_lag: integer("LinkLag")?.unwrap_or(0),
        lag_format,
    })
}

fn exclusion(node: Node) -> Option<String> {
    if flag(node, "IsNull") {
        Some("blank task row".into())
    } else if text(node, "Active").is_some_and(|v| v.trim() == "0" || v.trim() == "false") {
        Some("inactive task; the source does not schedule it".into())
    } else if flag(node, "ExternalTask") {
        Some("external task from another project".into())
    } else if flag(node, "IsSubproject") {
        Some("inserted subproject".into())
    } else {
        None
    }
}

pub(crate) fn children<'a, 'input>(
    node: Node<'a, 'input>,
    name: &'static str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

pub(crate) fn text<'a>(node: Node<'a, '_>, name: &'static str) -> Option<&'a str> {
    children(node, name)
        .next()
        .map(|c| c.text().unwrap_or_default())
}

pub(crate) fn flag(node: Node, name: &'static str) -> bool {
    text(node, name).is_some_and(|v| matches!(v.trim(), "1" | "true"))
}

/// An absent, empty or nil GUID names nothing; any other value must parse.
fn optional_guid(text: Option<&str>) -> Result<Option<Uuid>, String> {
    match text.map(str::trim).filter(|t| !t.is_empty()) {
        None => Ok(None),
        Some(value) => Uuid::parse_str(value)
            .map(|id| (!id.is_nil()).then_some(id))
            .map_err(|_| format!("GUID {value:?} is not a GUID")),
    }
}
