//! Field-by-field mapping of one source task onto candidate work.
//!
//! A field the source omits or leaves empty is not source data: existing work keeps its local
//! value and the report lists the field as `kept`, never as `preserved`. A field the import was
//! asked to keep is `kept` too, with the differing source value reported as approximated.

use super::{FieldPolicy, Parent, Placement};
use crate::mspdi::encoding::{
    DEFAULT_PRIORITY, TimeBasis, duration_seconds, estimate_from_seconds, format_guid, hours,
    parse_duration, priority_from_value, priority_value, time_basis,
};
use crate::mspdi::report::{Finding, ItemOutcome, ItemReport};
use crate::mspdi::source::SourceTask;
use dpm_model::{Key, ProjectId, WorkItem, WorkItemId, WorkKind, WorkStatus};
use std::collections::BTreeSet;

pub(super) fn build(
    task: &SourceTask,
    existing: Option<&WorkItem>,
    (id, key): (WorkItemId, Key),
    placement: &Placement,
    policy: FieldPolicy,
) -> (WorkItem, ItemReport) {
    let mut report = ItemReport {
        uid: task.uid,
        guid: task.guid.map(format_guid),
        name: task.name.clone(),
        outcome: ItemOutcome::Created,
        work: None,
        preserved: Vec::new(),
        approximated: Vec::new(),
        rejected: task.rejected.clone(),
        kept: Vec::new(),
        changes: Vec::new(),
    };
    let kind = map_kind(task, existing, placement.has_children, &mut report);
    let mut work = existing
        .cloned()
        .unwrap_or_else(|| new_work(id, key, kind, placement.project));
    work.kind = kind;
    work.parent = match placement.parent {
        Parent::Imported(parent) => Some(parent),
        Parent::TopLevel | Parent::NotImported(_) => None,
    };
    report.preserved.push("outline".into());
    map_title(task, existing, &mut work, &mut report);
    map_objective(task, existing, &mut work, &mut report);
    match existing {
        Some(existing) if policy.keep_existing_priority => {
            keep_priority(task, existing, &mut report);
        }
        _ => map_priority(task, existing, &mut work, &mut report),
    }
    map_duration(task, existing, &mut work, &mut report);
    (work, report)
}

/// A summary (or any task with children) is a work package; an absent milestone flag keeps an
/// existing milestone rather than turning it into a task.
pub(super) fn resolve_kind(
    task: &SourceTask,
    existing: Option<&WorkItem>,
    has_children: bool,
) -> WorkKind {
    if has_children || task.summary {
        WorkKind::WorkPackage
    } else {
        match task.milestone {
            Some(true) => WorkKind::Milestone,
            Some(false) => WorkKind::Task,
            None if existing.is_some_and(|e| e.kind == WorkKind::Milestone) => WorkKind::Milestone,
            None => WorkKind::Task,
        }
    }
}

fn map_kind(
    task: &SourceTask,
    existing: Option<&WorkItem>,
    has_children: bool,
    report: &mut ItemReport,
) -> WorkKind {
    let kind = resolve_kind(task, existing, has_children);
    // The outline and summary flag state a work package; otherwise only <Milestone> is source data.
    if kind == WorkKind::WorkPackage || task.milestone.is_some() {
        report.preserved.push("kind".into());
    } else if existing.is_some() {
        report.kept.push("kind".into());
    } else {
        report.approximated.push(Finding::new(
            "kind",
            format!("source omits Milestone; new work defaults to {kind:?}"),
        ));
    }
    kind
}

fn new_work(id: WorkItemId, key: Key, kind: WorkKind, project: ProjectId) -> WorkItem {
    WorkItem {
        id,
        key,
        project,
        parent: None,
        kind,
        title: String::new(),
        order: Default::default(),
        contract: dpm_model::WorkContract {
            objective: String::new(),
            acceptance: Vec::new(),
            instructions: None,
            capabilities: BTreeSet::new(),
            requirement_ids: BTreeSet::new(),
            assets: Vec::new(),
            condition: None,
            join: dpm_model::JoinPolicy::default(),
        },
        execution: dpm_model::ExecutionRecord {
            // MSPDI has no acceptance criteria, so imported tasks wait for ratification.
            status: if kind == WorkKind::Task {
                WorkStatus::Proposed
            } else {
                WorkStatus::Planned
            },
            reported_progress_percent: 0,
            artifact_ids: BTreeSet::new(),
            owner: None,
            handoffs: Vec::new(),
            releases: Vec::new(),
            last_rejection: None,
            attempts: Vec::new(),
            basis: Vec::new(),
            block_reason: None,
            events: Default::default(),
        },
        schedule: dpm_model::ScheduleInputs {
            priority: dpm_model::Priority::default(),
            estimate: None,
        },
    }
}

fn map_title(
    task: &SourceTask,
    existing: Option<&WorkItem>,
    work: &mut WorkItem,
    report: &mut ItemReport,
) {
    if !task.name.trim().is_empty() {
        work.title = task.name.clone();
        report.preserved.push("title".into());
    } else if existing.is_some() {
        report.kept.push("title".into());
    } else {
        work.title = format!("MSPDI task {}", task.uid);
        report
            .approximated
            .push(Finding::new("title", "source name is empty; titled by UID"));
    }
}

fn map_objective(
    task: &SourceTask,
    existing: Option<&WorkItem>,
    work: &mut WorkItem,
    report: &mut ItemReport,
) {
    if let Some(notes) = &task.notes {
        if !existing.is_some_and(|e| e.contract.objective.trim_end() == notes) {
            work.contract.objective = notes.clone();
        }
        report.preserved.push("notes".into());
    } else if existing.is_some() {
        report.kept.push("notes".into());
    }
}

/// Existing work keeps its priority by request; a differing source value stays visible.
fn keep_priority(task: &SourceTask, existing: &WorkItem, report: &mut ItemReport) {
    match task.priority {
        Some(value) if priority_value(existing.schedule.priority) == value => {
            report.preserved.push("priority".into());
        }
        Some(value) => {
            report.kept.push("priority".into());
            report.approximated.push(Finding::new(
                "priority",
                format!(
                    "source priority {value} not applied; existing {:?} kept",
                    existing.schedule.priority
                ),
            ));
        }
        None => report.kept.push("priority".into()),
    }
}

fn map_priority(
    task: &SourceTask,
    existing: Option<&WorkItem>,
    work: &mut WorkItem,
    report: &mut ItemReport,
) {
    let Some(value) = task.priority else {
        if existing.is_some() {
            report.kept.push("priority".into());
        } else {
            work.schedule.priority = priority_from_value(DEFAULT_PRIORITY);
            report.approximated.push(Finding::new(
                "priority",
                format!(
                    "source omits Priority; the MSPDI default {DEFAULT_PRIORITY} applies ({:?})",
                    work.schedule.priority
                ),
            ));
        }
        return;
    };
    if existing.is_some_and(|e| priority_value(e.schedule.priority) == value) {
        report.preserved.push("priority".into());
        return;
    }
    work.schedule.priority = priority_from_value(value);
    if priority_value(work.schedule.priority) == value {
        report.preserved.push("priority".into());
    } else {
        report.approximated.push(Finding::new(
            "priority",
            format!(
                "source priority {value} mapped to {:?}",
                work.schedule.priority
            ),
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
            if work.schedule.estimate.take().is_some() {
                report.approximated.push(Finding::new(
                    "duration",
                    "work packages carry no estimate; the local estimate is removed",
                ));
            }
            if task.milestone == Some(true) {
                report.approximated.push(Finding::new(
                    "milestone",
                    "milestone flag on a summary task ignored; summaries import as work packages",
                ));
            }
        }
        WorkKind::Milestone => {
            work.schedule.estimate = None;
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
        WorkKind::Task => match seconds {
            Some(seconds) => task_estimate(task, seconds, existing, work, report),
            None if existing.is_some_and(|e| e.kind == WorkKind::Task) => {
                report.kept.push("duration".into());
            }
            None => {}
        },
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
        .is_some_and(|e| duration_seconds(e.schedule.estimate) == seconds);
    if unchanged || (seconds == 0 && work.schedule.estimate.is_none()) {
        report.preserved.push("duration".into());
        return;
    }
    if seconds == 0 {
        work.schedule.estimate = None;
        report.approximated.push(Finding::new(
            "duration",
            "zero source duration removes the local estimate; the task becomes unestimated",
        ));
        return;
    }
    work.schedule.estimate = estimate_from_seconds(seconds);
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
