//! Where the schedule places each work item: its own range, or a package's descendants.

use dpm_model::{Plan, Timeline, WorkItem, WorkItemId, WorkKind};
use dpm_schedule::{ActivitySchedule, Schedule};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Scheduled range of one work item in elapsed hours, with whether the critical path touches it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScheduleSpan {
    /// Earliest start of the work, or of the earliest descendant of a package.
    pub start_hours: f64,
    /// Earliest finish of the work, or of the latest descendant of a package.
    pub finish_hours: f64,
    /// Whether the work, or any descendant of a package, is critical.
    pub critical: bool,
}

type Children<'a> = BTreeMap<WorkItemId, Vec<&'a WorkItem>>;

/// Scheduled range of `item`, or `None` for work the timeline places outside the active graph;
/// packages span their applicable descendants. This is the rule the terminal Gantt draws.
#[must_use]
pub fn schedule_span(
    plan: &Plan,
    timeline: &Timeline,
    schedule: &Schedule,
    item: &WorkItem,
) -> Option<ScheduleSpan> {
    if item.kind != WorkKind::WorkPackage {
        return leaf(timeline, schedule, item);
    }
    package(timeline, schedule, &children(plan), item)
}

/// Spans of every work item the active graph places.
pub(super) fn spans(
    plan: &Plan,
    timeline: &Timeline,
    schedule: &Schedule,
) -> BTreeMap<WorkItemId, ScheduleSpan> {
    let children = children(plan);
    plan.work_items
        .values()
        .filter_map(|item| {
            let span = if item.kind == WorkKind::WorkPackage {
                package(timeline, schedule, &children, item)
            } else {
                leaf(timeline, schedule, item)
            };
            span.map(|span| (item.id, span))
        })
        .collect()
}

/// Outstanding leaf work below each applicable package that has applicable leaf work, through
/// applicable nested packages. A package's own activity is an isolated zero-duration node whose
/// float spans the whole project, so its rolled-up times and criticality come from these instead.
/// Completed work stays in the remaining schedule as a zero-duration node at the origin whose
/// latest finish is the project finish, so it is left out; a package whose work is all complete
/// maps to no leaves and reports no times.
pub(super) fn package_leaves(
    plan: &Plan,
    timeline: &Timeline,
    schedule: &Schedule,
) -> BTreeMap<WorkItemId, Vec<WorkItemId>> {
    let children = children(plan);
    let completed = timeline.completed();
    plan.work_items
        .values()
        .filter(|item| item.kind == WorkKind::WorkPackage)
        .filter(|item| timeline.applicability(item.id).is_applicable())
        .filter_map(|item| {
            let mut leaves = Vec::new();
            let found = Leaves {
                timeline,
                schedule,
                children: &children,
                completed: &completed,
            }
            .collect(item.id, &mut leaves);
            found.then_some((item.id, leaves))
        })
        .collect()
}

struct Leaves<'a> {
    timeline: &'a Timeline,
    schedule: &'a Schedule,
    children: &'a Children<'a>,
    completed: &'a BTreeSet<WorkItemId>,
}

impl Leaves<'_> {
    /// Push the outstanding leaves below `package`; whether it has any applicable leaf at all.
    fn collect(&self, package: WorkItemId, out: &mut Vec<WorkItemId>) -> bool {
        let mut found = false;
        for child in self.children.get(&package).into_iter().flatten() {
            if !self.timeline.applicability(child.id).is_applicable() {
                continue;
            }
            if child.kind == WorkKind::WorkPackage {
                found |= self.collect(child.id, out);
            } else if self.schedule.activities.contains_key(&child.id) {
                found = true;
                if !self.completed.contains(&child.id) {
                    out.push(child.id);
                }
            }
        }
        found
    }
}

/// Earliest and latest bounds enclosing `leaves`, with their least float.
pub(super) fn rolled_up(schedule: &Schedule, leaves: &[WorkItemId]) -> Option<ActivitySchedule> {
    leaves
        .iter()
        .filter_map(|id| schedule.activities.get(id))
        .map(|a| ActivitySchedule {
            calendar: None,
            ..a.clone()
        })
        .reduce(|a, b| ActivitySchedule {
            earliest_start_hours: a.earliest_start_hours.min(b.earliest_start_hours),
            earliest_finish_hours: a.earliest_finish_hours.max(b.earliest_finish_hours),
            latest_start_hours: a.latest_start_hours.min(b.latest_start_hours),
            latest_finish_hours: a.latest_finish_hours.max(b.latest_finish_hours),
            total_float_hours: a.total_float_hours.min(b.total_float_hours),
            free_float_hours: a.free_float_hours.min(b.free_float_hours),
            critical: a.critical || b.critical,
            calendar: None,
        })
}

fn children(plan: &Plan) -> Children<'_> {
    let mut children = Children::new();
    for item in plan.work_items.values() {
        if let Some(parent) = item.parent {
            children.entry(parent).or_default().push(item);
        }
    }
    children
}

fn leaf(timeline: &Timeline, schedule: &Schedule, item: &WorkItem) -> Option<ScheduleSpan> {
    if !timeline.applicability(item.id).is_applicable() {
        return None;
    }
    let activity = schedule.activities.get(&item.id)?;
    Some(ScheduleSpan {
        start_hours: activity.earliest_start_hours,
        finish_hours: activity.earliest_finish_hours,
        critical: activity.critical,
    })
}

fn package(
    timeline: &Timeline,
    schedule: &Schedule,
    children: &Children<'_>,
    item: &WorkItem,
) -> Option<ScheduleSpan> {
    if !timeline.applicability(item.id).is_applicable() {
        return None;
    }
    let Some(own) = children.get(&item.id) else {
        return Some(ScheduleSpan {
            start_hours: 0.0,
            finish_hours: 0.0,
            critical: false,
        });
    };
    own.iter()
        .filter_map(|child| {
            if child.kind == WorkKind::WorkPackage {
                package(timeline, schedule, children, child)
            } else {
                leaf(timeline, schedule, child)
            }
        })
        .reduce(|a, b| ScheduleSpan {
            start_hours: a.start_hours.min(b.start_hours),
            finish_hours: a.finish_hours.max(b.finish_hours),
            critical: a.critical || b.critical,
        })
}
