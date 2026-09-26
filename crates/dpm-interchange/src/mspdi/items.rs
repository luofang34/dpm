//! Map source tasks onto candidate work items, keeping each item's report beside it.

use super::encoding::{
    DEFAULT_PRIORITY, TimeBasis, derived_work_id, duration_seconds, estimate_from_seconds,
    format_guid, hours, parse_duration, priority_from_value, priority_value, time_basis,
};
use super::report::{Finding, ItemOutcome, ItemReport, WorkReference};
use super::source::{SourceProject, SourceTask};
use crate::InterchangeError;
use dpm_model::{Key, Plan, ProjectId, WorkItem, WorkItemId, WorkKind, WorkStatus};
use std::collections::{BTreeMap, BTreeSet};

/// Candidate work for every imported task, with the per-task reports in document order.
#[derive(Debug, Default)]
pub(crate) struct Outline {
    pub(crate) reports: Vec<ItemReport>,
    work: BTreeMap<i64, WorkItem>,
    skipped: BTreeSet<i64>,
}

impl Outline {
    pub(crate) fn mapped(&self) -> impl Iterator<Item = &WorkItem> {
        self.work.values()
    }

    pub(crate) fn contains(&self, id: WorkItemId) -> bool {
        self.work.values().any(|w| w.id == id)
    }

    /// Candidate work for a source UID; `Err` explains why a link cannot use it.
    pub(crate) fn work(&self, uid: i64) -> Result<&WorkItem, String> {
        self.work.get(&uid).ok_or_else(|| {
            if self.skipped.contains(&uid) {
                format!("task UID {uid} is not imported")
            } else {
                format!("task UID {uid} is not in the document")
            }
        })
    }

    /// Imported task and milestone descendants of a work package, in identity order.
    pub(crate) fn leaves(&self, package: WorkItemId) -> Vec<WorkItemId> {
        let mut found = Vec::new();
        let mut pending = vec![package];
        while let Some(parent) = pending.pop() {
            for child in self.work.values().filter(|w| w.parent == Some(parent)) {
                match child.kind {
                    WorkKind::WorkPackage => pending.push(child.id),
                    WorkKind::Task | WorkKind::Milestone => found.push(child.id),
                }
            }
        }
        found.sort();
        found
    }
}

/// Where a source task sits relative to the imported outline.
enum Parent {
    TopLevel,
    Imported(WorkItemId),
    NotImported(i64),
}

pub(crate) fn map(
    current: &Plan,
    source: &SourceProject,
    project: ProjectId,
    prefix: &str,
) -> Result<Outline, InterchangeError> {
    let mut outline = Outline::default();
    let mut identities: BTreeMap<WorkItemId, i64> = BTreeMap::new();
    let mut keys: BTreeSet<String> = current
        .work_items
        .values()
        .map(|w| w.key.0.clone())
        .collect();
    let mut stack: Vec<(u32, i64)> = Vec::new();
    for (index, task) in source.tasks.iter().enumerate() {
        if task.is_project_summary() {
            outline.skipped.insert(task.uid);
            outline.reports.push(skipped(
                task,
                "project_summary",
                "summarizes the whole document; the target project stands for it",
            ));
            continue;
        }
        if outline.work.contains_key(&task.uid) || outline.skipped.contains(&task.uid) {
            return Err(InterchangeError::DuplicateUid { uid: task.uid });
        }
        while stack
            .last()
            .is_some_and(|(level, _)| *level >= task.outline_level)
        {
            stack.pop();
        }
        if stack.len() + 1 != task.outline_level as usize {
            return Err(InterchangeError::MalformedTask {
                position: task.position,
                reason: format!(
                    "task UID {} jumps to outline level {}",
                    task.uid, task.outline_level
                ),
            });
        }
        let parent = match stack.last() {
            None => Parent::TopLevel,
            Some((_, uid)) => outline
                .work
                .get(uid)
                .map_or(Parent::NotImported(*uid), |w| Parent::Imported(w.id)),
        };
        stack.push((task.outline_level, task.uid));
        let has_children = source
            .tasks
            .get(index + 1)
            .is_some_and(|next| next.outline_level > task.outline_level);
        let placement = Placement {
            project,
            parent,
            has_children,
        };
        match place(current, source, task, &placement)? {
            Err(report) => {
                outline.skipped.insert(task.uid);
                outline.reports.push(report);
            }
            Ok((id, existing)) => {
                if let Some(first) = identities.insert(id, task.uid) {
                    return Err(InterchangeError::DuplicateIdentity {
                        first,
                        second: task.uid,
                        identity: id.to_string(),
                    });
                }
                let key = key_for(existing, task, prefix, &mut keys)?;
                let (work, report) = build(task, existing, id, key, &placement);
                outline.work.insert(task.uid, work);
                outline.reports.push(report);
            }
        }
    }
    keep_outer_parents(current, &mut outline);
    finish_reports(current, &mut outline);
    Ok(outline)
}

struct Placement {
    project: ProjectId,
    parent: Parent,
    has_children: bool,
}

/// Resolve identity and ownership, or the skipped-item report explaining why the task stays out.
fn place<'a>(
    current: &'a Plan,
    source: &SourceProject,
    task: &SourceTask,
    placement: &Placement,
) -> Result<Result<(WorkItemId, Option<&'a WorkItem>), ItemReport>, InterchangeError> {
    if let Some(reason) = &task.exclusion {
        return Ok(Err(skipped(task, "task", reason)));
    }
    if let Parent::NotImported(uid) = placement.parent {
        return Ok(Err(skipped(
            task,
            "parent",
            format!("summary task UID {uid} is not imported"),
        )));
    }
    let id = match (task.guid, source.guid) {
        (Some(guid), _) => WorkItemId(guid),
        (None, Some(project)) => derived_work_id(project, task.uid),
        (None, None) => {
            return Ok(Err(skipped(
                task,
                "identity",
                "neither the task nor the project has a GUID, so re-imports could not find it",
            )));
        }
    };
    let existing = current.work_items.get(&id);
    if let Some(work) = existing.filter(|w| w.project != placement.project) {
        return Ok(Err(skipped(
            task,
            "identity",
            format!("identity {id} belongs to {} in another project", work.key),
        )));
    }
    Ok(Ok((id, existing)))
}

fn key_for(
    existing: Option<&WorkItem>,
    task: &SourceTask,
    prefix: &str,
    keys: &mut BTreeSet<String>,
) -> Result<Key, InterchangeError> {
    if let Some(work) = existing {
        return Ok(work.key.clone());
    }
    let key = format!("{prefix}-{}", task.uid);
    if !keys.insert(key.clone()) {
        return Err(InterchangeError::KeyCollision { key, uid: task.uid });
    }
    Ok(Key(key))
}

fn build(
    task: &SourceTask,
    existing: Option<&WorkItem>,
    id: WorkItemId,
    key: Key,
    placement: &Placement,
) -> (WorkItem, ItemReport) {
    let kind = if placement.has_children || task.summary {
        WorkKind::WorkPackage
    } else if task.milestone {
        WorkKind::Milestone
    } else {
        WorkKind::Task
    };
    let mut report = ItemReport {
        uid: task.uid,
        guid: task.guid.map(format_guid),
        name: task.name.clone(),
        outcome: ItemOutcome::Created,
        work: None,
        preserved: vec!["kind".into(), "outline".into()],
        approximated: Vec::new(),
        rejected: task.rejected.clone(),
    };
    if task.guid.is_some() {
        report.preserved.insert(0, "identity".into());
    } else {
        report.approximated.push(Finding::new(
            "identity",
            format!(
                "no task GUID; identity derived from the project GUID and UID {}, so a renumbered UID imports as new work",
                task.uid
            ),
        ));
    }
    let mut work = existing
        .cloned()
        .unwrap_or_else(|| new_work(id, key, kind, placement.project));
    work.kind = kind;
    work.parent = match placement.parent {
        Parent::Imported(parent) => Some(parent),
        Parent::TopLevel | Parent::NotImported(_) => None,
    };
    map_title(task, &mut work, &mut report);
    map_objective(task, &mut work, &mut report);
    map_priority(task, existing, &mut work, &mut report);
    map_duration(task, existing, &mut work, &mut report);
    (work, report)
}

fn new_work(id: WorkItemId, key: Key, kind: WorkKind, project: ProjectId) -> WorkItem {
    WorkItem {
        id,
        key,
        project,
        parent: None,
        kind,
        title: String::new(),
        objective: String::new(),
        acceptance: Vec::new(),
        instructions: None,
        // MSPDI has no acceptance criteria, so imported tasks wait for ratification.
        status: if kind == WorkKind::Task {
            WorkStatus::Proposed
        } else {
            WorkStatus::Planned
        },
        reported_progress_percent: 0,
        priority: dpm_model::Priority::default(),
        estimate: None,
        capabilities: BTreeSet::new(),
        requirement_ids: BTreeSet::new(),
        artifact_ids: BTreeSet::new(),
        owner: None,
        resources: Vec::new(),
        last_rejection: None,
        attempts: Vec::new(),
        basis: Vec::new(),
        block_reason: None,
        events: Default::default(),
        condition: None,
        join: dpm_model::JoinPolicy::default(),
    }
}

fn map_title(task: &SourceTask, work: &mut WorkItem, report: &mut ItemReport) {
    if task.name.trim().is_empty() {
        work.title = format!("MSPDI task {}", task.uid);
        report
            .approximated
            .push(Finding::new("title", "source name is empty; titled by UID"));
    } else {
        work.title = task.name.clone();
        report.preserved.push("title".into());
    }
}

fn map_objective(task: &SourceTask, work: &mut WorkItem, report: &mut ItemReport) {
    if let Some(notes) = &task.notes {
        work.objective = notes.clone();
        report.preserved.push("notes".into());
    }
}

fn map_priority(
    task: &SourceTask,
    existing: Option<&WorkItem>,
    work: &mut WorkItem,
    report: &mut ItemReport,
) {
    let value = task.priority.unwrap_or(DEFAULT_PRIORITY);
    if existing.is_some_and(|e| priority_value(e.priority) == value) {
        report.preserved.push("priority".into());
        return;
    }
    work.priority = priority_from_value(value);
    if priority_value(work.priority) == value {
        report.preserved.push("priority".into());
    } else {
        report.approximated.push(Finding::new(
            "priority",
            format!("source priority {value} mapped to {:?}", work.priority),
        ));
    }
}

fn map_duration(
    task: &SourceTask,
    existing: Option<&WorkItem>,
    work: &mut WorkItem,
    report: &mut ItemReport,
) {
    let seconds = match task.duration.as_deref().map(parse_duration) {
        None => None,
        Some(Ok(seconds)) => Some(seconds),
        Some(Err(reason)) => {
            report.rejected.push(Finding::new("duration", reason));
            None
        }
    };
    match work.kind {
        WorkKind::WorkPackage => {
            work.estimate = None;
            if task.milestone {
                report.approximated.push(Finding::new(
                    "milestone",
                    "milestone flag on a summary task ignored; summaries import as work packages",
                ));
            }
        }
        WorkKind::Milestone => {
            work.estimate = None;
            if let Some(seconds) = seconds.filter(|s| *s > 0) {
                report.approximated.push(Finding::new(
                    "duration",
                    format!(
                        "milestone duration {} h dropped; milestones have zero duration",
                        hours(seconds)
                    ),
                ));
            }
        }
        WorkKind::Task => {
            let Some(seconds) = seconds else { return };
            task_estimate(task, seconds, existing, work, report);
        }
    }
}

fn task_estimate(
    task: &SourceTask,
    seconds: u64,
    existing: Option<&WorkItem>,
    work: &mut WorkItem,
    report: &mut ItemReport,
) {
    let unchanged = existing
        .filter(|e| e.kind == WorkKind::Task)
        .is_some_and(|e| duration_seconds(e.estimate) == seconds);
    if unchanged || (seconds == 0 && work.estimate.is_none()) {
        report.preserved.push("duration".into());
        return;
    }
    if seconds == 0 {
        work.estimate = None;
        report.approximated.push(Finding::new(
            "duration",
            "zero source duration removes the local estimate; the task becomes unestimated",
        ));
        return;
    }
    work.estimate = estimate_from_seconds(seconds);
    let basis = task.duration_format.and_then(time_basis);
    let detail = match basis {
        Some(TimeBasis::Elapsed) => format!(
            "elapsed duration {} h becomes the single-point estimate O=M=P",
            hours(seconds)
        ),
        _ => format!(
            "working-time duration {} h treated as elapsed hours (calendar not applied); single-point estimate O=M=P",
            hours(seconds)
        ),
    };
    report.approximated.push(Finding::new("duration", detail));
}

/// A top-level source task keeps a local parent that the document does not describe, so importing
/// a sub-schedule under an existing package does not detach it.
fn keep_outer_parents(current: &Plan, outline: &mut Outline) {
    let imported: BTreeSet<_> = outline.work.values().map(|w| w.id).collect();
    for work in outline.work.values_mut() {
        if work.parent.is_none()
            && let Some(parent) = current.work_items.get(&work.id).and_then(|e| e.parent)
            && !imported.contains(&parent)
        {
            work.parent = Some(parent);
        }
    }
}

fn finish_reports(current: &Plan, outline: &mut Outline) {
    for report in &mut outline.reports {
        let Some(work) = outline.work.get(&report.uid) else {
            continue;
        };
        report.outcome = match current.work_items.get(&work.id) {
            None => ItemOutcome::Created,
            Some(existing) if existing == work => ItemOutcome::Unchanged,
            Some(_) => ItemOutcome::Updated,
        };
        report.work = Some(WorkReference {
            id: work.id,
            key: work.key.clone(),
            kind: work.kind,
            status: work.status,
        });
    }
}

fn skipped(task: &SourceTask, field: &str, reason: impl Into<String>) -> ItemReport {
    let mut rejected = vec![Finding::new(field, reason)];
    rejected.extend(task.rejected.iter().cloned());
    ItemReport {
        uid: task.uid,
        guid: task.guid.map(format_guid),
        name: task.name.clone(),
        outcome: ItemOutcome::Skipped,
        work: None,
        preserved: Vec::new(),
        approximated: Vec::new(),
        rejected,
    }
}
