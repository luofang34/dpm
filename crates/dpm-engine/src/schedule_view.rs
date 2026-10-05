//! The shared read-only schedule projection: the deterministic remaining-work schedule, package
//! spans and the seeded simulation's criticality and percentiles, as every client draws them.

use crate::EngineError;
use chrono::{DateTime, Utc};
use dpm_model::{
    Applicability, DependencyId, DependencyKind, Key, Plan, Priority, Timeline, WorkItem,
    WorkItemId, WorkKind, WorkStatus,
};
use dpm_schedule::{
    ActivityCalendar, ActivitySchedule, Schedule, SimulationSummary, deterministic_remaining_at,
    simulate_remaining_at,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod span;
pub use span::{ScheduleSpan, schedule_span};

#[cfg(test)]
mod tests;

/// Critical-path times of one work item the active graph schedules, in elapsed hours after the
/// evaluation time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTimes {
    /// Minimum feasible start.
    pub earliest_start_hours: f64,
    /// Earliest start plus duration.
    pub earliest_finish_hours: f64,
    /// Latest start that does not delay project completion.
    pub latest_start_hours: f64,
    /// Latest finish that does not delay project completion.
    pub latest_finish_hours: f64,
    /// Delay available without postponing project completion.
    pub total_float_hours: f64,
    /// Delay available without moving a direct successor's earliest start.
    pub free_float_hours: f64,
    /// Whether total float is within numerical tolerance of zero.
    pub critical: bool,
}

/// One row of the schedule projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledWork {
    /// Stable identity.
    pub id: WorkItemId,
    /// Human-readable key.
    pub key: Key,
    /// Task, work package or milestone.
    pub kind: WorkKind,
    /// Containing work package.
    pub parent: Option<WorkItemId>,
    /// Title.
    pub title: String,
    /// Lifecycle status, with aggregate completion reflected for containers and milestones.
    pub status: WorkStatus,
    /// Whether the work is in the active graph, and why not.
    pub applicability: Applicability,
    /// Human priority; separate from criticality.
    pub priority: Priority,
    /// What the Gantt draws: the activity's own range, or a package's applicable descendants;
    /// absent for work outside the active graph.
    pub span: Option<ScheduleSpan>,
    /// Critical-path times; a package with applicable descendants reports the enclosing bounds and
    /// least float of those still outstanding. Absent for work outside the active graph and for a
    /// package whose work is all complete.
    pub times: Option<ScheduledTimes>,
    /// Fraction of simulated schedules in which the work is critical, or for a package the
    /// largest fraction among its descendants; absent (never zero) when
    /// the projection is deterministic only, while open choices make it meaningless, or for work
    /// outside the active graph.
    pub criticality: Option<f64>,
    /// Calendar placement, present only when the plan has calendars.
    pub calendar: Option<ActivityCalendar>,
}

/// Seeded Monte Carlo completion percentiles, in elapsed hours.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleUncertainty {
    /// Number of sampled schedules.
    pub iterations: usize,
    /// Pseudo-random seed.
    pub seed: u64,
    /// Median completion.
    pub p50_finish_hours: f64,
    /// 80th percentile completion.
    pub p80_finish_hours: f64,
    /// 95th percentile completion.
    pub p95_finish_hours: f64,
}

/// The authoritative remaining-work schedule projection of one snapshot at one clock reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleProjection {
    /// Deterministic remaining duration of the active graph, in elapsed hours.
    pub project_finish_hours: f64,
    /// Percentiles; absent for a deterministic-only request, while open choices make them
    /// meaningless, or when no work has an estimate.
    pub uncertainty: Option<ScheduleUncertainty>,
    /// Every work item in key order.
    pub work: Vec<ScheduledWork>,
    /// Constraints that still bound outstanding work, in plan order; satisfied, waived and
    /// inapplicable ones are absent.
    pub relations: Vec<ScheduledRelation>,
}

/// One outstanding constraint and how tightly it binds its successor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledRelation {
    /// Stable dependency identity.
    pub id: DependencyId,
    /// Work providing the constrained event.
    pub predecessor: WorkItemId,
    /// Work whose event is bounded.
    pub successor: WorkItemId,
    /// FS, SS, FF or SF.
    pub kind: DependencyKind,
    /// How far the successor's start lies past what this constraint alone requires; zero when it
    /// drives.
    pub slack_hours: f64,
    /// This constraint sets its successor's earliest start, or on calendars holds its finish.
    pub driving: bool,
    /// Driving between two critical activities, so it lies on a critical path.
    pub critical: bool,
}

/// Project the remaining schedule; `probabilistic` adds the simulation `status` and `next` use.
pub fn schedule_projection(
    plan: &Plan,
    probabilistic: bool,
    now: DateTime<Utc>,
) -> Result<ScheduleProjection, EngineError> {
    plan.validate()?;
    let timeline = Timeline::at(plan, now);
    let schedule = deterministic_remaining_at(plan, &timeline)?;
    let config = crate::query::simulation_config();
    let simulation = if probabilistic
        && !crate::query::choices::has_open_choices(plan)
        && plan
            .work_items
            .values()
            .any(|w| w.schedule.estimate.is_some())
    {
        Some(simulate_remaining_at(plan, config, &timeline)?)
    } else {
        None
    };
    let spans = span::spans(plan, &timeline, &schedule);
    let leaves = span::package_leaves(plan, &timeline, &schedule);
    let mut work: Vec<ScheduledWork> = plan
        .work_items
        .values()
        .map(|item| {
            let mut scheduled = row(&timeline, &schedule, simulation.as_ref(), &spans, item);
            if let Some(leaves) = leaves.get(&item.id) {
                roll_up(&mut scheduled, &schedule, simulation.as_ref(), leaves);
            }
            scheduled
        })
        .collect();
    work.sort_by(|a, b| a.key.natural_cmp(&b.key));
    Ok(ScheduleProjection {
        project_finish_hours: schedule.project_finish_hours,
        uncertainty: simulation.map(|s| ScheduleUncertainty {
            iterations: s.iterations,
            seed: config.seed,
            p50_finish_hours: s.p50_finish_hours,
            p80_finish_hours: s.p80_finish_hours,
            p95_finish_hours: s.p95_finish_hours,
        }),
        work,
        relations: relations(plan, &schedule),
    })
}

fn relations(plan: &Plan, schedule: &Schedule) -> Vec<ScheduledRelation> {
    plan.dependencies
        .iter()
        .filter_map(|d| {
            let r = schedule.relations.get(&d.id)?;
            Some(ScheduledRelation {
                id: d.id,
                predecessor: d.predecessor,
                successor: d.successor,
                kind: d.kind,
                slack_hours: r.slack_hours,
                driving: r.driving,
                critical: r.critical,
            })
        })
        .collect()
}

/// A package reports its descendants' bounds, least float, and the criticality of its most
/// critical descendant, never the meaningless float of its own isolated node.
fn roll_up(
    scheduled: &mut ScheduledWork,
    schedule: &Schedule,
    simulation: Option<&SimulationSummary>,
    leaves: &[WorkItemId],
) {
    scheduled.times = span::rolled_up(schedule, leaves).map(|a| times(&a));
    scheduled.criticality = simulation.and_then(|s| {
        leaves
            .iter()
            .filter_map(|id| s.criticality.get(id).copied())
            .reduce(f64::max)
    });
}

fn times(a: &ActivitySchedule) -> ScheduledTimes {
    ScheduledTimes {
        earliest_start_hours: a.earliest_start_hours,
        earliest_finish_hours: a.earliest_finish_hours,
        latest_start_hours: a.latest_start_hours,
        latest_finish_hours: a.latest_finish_hours,
        total_float_hours: a.total_float_hours,
        free_float_hours: a.free_float_hours,
        critical: a.critical,
    }
}

fn row(
    timeline: &Timeline,
    schedule: &Schedule,
    simulation: Option<&SimulationSummary>,
    spans: &BTreeMap<WorkItemId, ScheduleSpan>,
    item: &WorkItem,
) -> ScheduledWork {
    let applicability = timeline.applicability(item.id);
    let activity = schedule
        .activities
        .get(&item.id)
        .filter(|_| applicability.is_applicable());
    let mut status = item.execution.status;
    if !item.is_executable() && timeline.completed_at(item.id).is_some() {
        status = WorkStatus::Verified;
    }
    ScheduledWork {
        id: item.id,
        key: item.key.clone(),
        kind: item.kind,
        parent: item.parent,
        title: item.title.clone(),
        status,
        applicability: applicability.clone(),
        priority: item.schedule.priority,
        span: spans.get(&item.id).copied(),
        times: activity.map(times),
        criticality: simulation.and_then(|s| s.criticality.get(&item.id).copied()),
        calendar: activity.and_then(|a| a.calendar.clone()),
    }
}
