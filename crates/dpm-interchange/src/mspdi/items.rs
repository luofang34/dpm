//! Map source tasks onto candidate work items, keeping each item's report beside it.

mod changes;
mod fields;
mod identity;

pub(crate) use identity::Resolver;

use super::encoding::format_guid;
use super::report::{Finding, ItemOutcome, ItemReport, WorkReference};
use super::source::{SourceProject, SourceTask};
use crate::InterchangeError;
use dpm_model::{Key, Plan, ProjectId, WorkItem, WorkItemId, WorkKind};
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
    resolver: &Resolver,
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
        match place(current, resolver, task, &placement) {
            Err(report) => {
                outline.skipped.insert(task.uid);
                outline.reports.push(*report);
            }
            Ok((id, basis, existing)) => {
                if let Some(first) = identities.insert(id, task.uid) {
                    return Err(InterchangeError::DuplicateIdentity {
                        first,
                        second: task.uid,
                        identity: id.to_string(),
                    });
                }
                check_kind(task, existing, placement.has_children)?;
                let key = key_for(existing, task, prefix, &mut keys)?;
                let (work, mut report) = fields::build(task, existing, id, key, &placement);
                resolver.record(task, &basis, &mut report);
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
    resolver: &Resolver,
    task: &SourceTask,
    placement: &Placement,
) -> Result<Placed<'a>, Box<ItemReport>> {
    if let Some(reason) = &task.exclusion {
        return Err(Box::new(skipped(task, "task", reason)));
    }
    if let Parent::NotImported(uid) = placement.parent {
        return Err(Box::new(skipped(
            task,
            "parent",
            format!("summary task UID {uid} is not imported"),
        )));
    }
    let (id, basis) = resolver.resolve(task);
    let existing = current.work_items.get(&id);
    if let Some(work) = existing.filter(|w| w.project != placement.project) {
        return Err(Box::new(skipped(
            task,
            "identity",
            format!("identity {id} belongs to {} in another project", work.key),
        )));
    }
    Ok((id, basis, existing))
}

/// Resolved identity, how it was found, and the existing work it names.
type Placed<'a> = (WorkItemId, identity::Basis, Option<&'a WorkItem>);

/// Review never changes the kind of existing work, so the importer names the source task and the
/// attempted change instead of leaving review to refuse the local key alone.
fn check_kind(
    task: &SourceTask,
    existing: Option<&WorkItem>,
    has_children: bool,
) -> Result<(), InterchangeError> {
    let Some(work) = existing else {
        return Ok(());
    };
    let kind = fields::resolve_kind(task, existing, has_children);
    if kind == work.kind {
        return Ok(());
    }
    Err(InterchangeError::KindChange {
        uid: task.uid,
        guid: task.guid.map(format_guid),
        key: work.key.0.clone(),
        from: work.kind,
        to: kind,
    })
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

/// A top-level source task keeps a local parent that the document does not describe, so importing
/// a sub-schedule under an existing package does not detach it.
fn keep_outer_parents(current: &Plan, outline: &mut Outline) {
    let imported: BTreeSet<_> = outline.work.values().map(|w| w.id).collect();
    let mut kept = BTreeSet::new();
    for (uid, work) in &mut outline.work {
        if work.parent.is_none()
            && let Some(parent) = current.work_items.get(&work.id).and_then(|e| e.parent)
            && !imported.contains(&parent)
        {
            work.parent = Some(parent);
            kept.insert(*uid);
        }
    }
    for report in outline.reports.iter_mut().filter(|r| kept.contains(&r.uid)) {
        report.preserved.retain(|field| field != "outline");
        report.kept.push("outline".into());
    }
}

fn finish_reports(current: &Plan, outline: &mut Outline) {
    let keys: BTreeMap<WorkItemId, &Key> = current
        .work_items
        .values()
        .chain(outline.work.values())
        .map(|w| (w.id, &w.key))
        .collect();
    for report in &mut outline.reports {
        let Some(work) = outline.work.get(&report.uid) else {
            continue;
        };
        report.outcome = match current.work_items.get(&work.id) {
            None => ItemOutcome::Created,
            Some(existing) => {
                report.changes = changes::between(existing, work, &keys);
                if existing == work {
                    ItemOutcome::Unchanged
                } else {
                    ItemOutcome::Updated
                }
            }
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
        kept: Vec::new(),
        changes: Vec::new(),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
