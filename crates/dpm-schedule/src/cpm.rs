use crate::{ActivitySchedule, Schedule, ScheduleError};
use dpm_model::{Dependency, DependencyKind, Plan, Release, WorkItemId};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Absolute tolerance, in hours, below which total float counts as zero.
///
/// Inputs are elapsed hours; at a million hours the f64 spacing is about 1e-10, so rounding from
/// sums of durations and lags stays at least two orders of magnitude below this bound while any
/// intentional slack (whole seconds and above) stays far above it.
const EPSILON: f64 = 1e-8;
type Times = BTreeMap<WorkItemId, f64>;

fn relation_weight(kind: DependencyKind, predecessor: f64, successor: f64, lag: f64) -> f64 {
    match kind {
        DependencyKind::FinishStart => predecessor + lag,
        DependencyKind::StartStart => lag,
        DependencyKind::FinishFinish => predecessor - successor + lag,
        DependencyKind::StartFinish => -successor + lag,
    }
}

fn topological_order(plan: &Plan) -> Result<Vec<WorkItemId>, ScheduleError> {
    let mut indegree: BTreeMap<_, usize> = plan.work_items.keys().map(|id| (*id, 0)).collect();
    let mut outgoing: BTreeMap<_, Vec<_>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for dep in &plan.dependencies {
        if !plan.work_items.contains_key(&dep.predecessor) {
            return Err(ScheduleError::MissingWorkItem(dep.predecessor));
        }
        if seen.insert((dep.predecessor, dep.successor)) {
            outgoing
                .entry(dep.predecessor)
                .or_default()
                .push(dep.successor);
            let degree = indegree
                .get_mut(&dep.successor)
                .ok_or(ScheduleError::MissingWorkItem(dep.successor))?;
            *degree = degree.wrapping_add(1);
        }
    }
    let mut queue: VecDeque<_> = indegree
        .iter()
        .filter_map(|(id, n)| (*n == 0).then_some(*id))
        .collect();
    let mut order = Vec::with_capacity(plan.work_items.len());
    while let Some(id) = queue.pop_front() {
        order.push(id);
        for child in outgoing.get(&id).into_iter().flatten() {
            let degree = indegree
                .get_mut(child)
                .ok_or(ScheduleError::MissingWorkItem(*child))?;
            *degree -= 1;
            if *degree == 0 {
                queue.push_back(*child);
            }
        }
    }
    if order.len() != plan.work_items.len() {
        return Err(ScheduleError::DependencyCycle);
    }
    Ok(order)
}

/// Project the full baseline using each task's weighted PERT duration.
pub fn deterministic(plan: &Plan) -> Result<Schedule, ScheduleError> {
    let durations = plan
        .work_items
        .iter()
        .map(|(id, work)| (*id, work.expected_duration_hours()))
        .collect();
    deterministic_with_durations(plan, &durations)
}

/// Project outstanding work, treating verified tasks as zero remaining duration.
///
/// Waived constraints no longer bound outstanding work; the baseline projection keeps them.
/// `now` is the adapter's clock reading, used only to decide which milestones are reached.
pub fn deterministic_remaining(
    plan: &Plan,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Schedule, ScheduleError> {
    plan.validate()?;
    let remaining = remaining_plan(plan, now);
    let plan = &remaining;
    let durations = plan
        .work_items
        .iter()
        .map(|(id, work)| {
            (
                *id,
                if work.status.satisfies_dependency() {
                    0.0
                } else {
                    work.expected_duration_hours()
                },
            )
        })
        .collect();
    deterministic_with_durations(plan, &durations)
}

/// Outstanding constraints measured from `now` as the projection origin.
///
/// Edges into completed work and waived edges no longer bound anything. An edge from completed
/// work keeps only the part of its lag the shared gate evaluator still reports as elapsing, so the
/// forecast waits exactly as long as execution will; a lag whose event time was never recorded is
/// kept whole rather than assumed to have elapsed. The completed predecessor projects at the origin
/// with zero duration, so the kept lag is measured from `now`.
pub(crate) fn remaining_plan(plan: &Plan, now: chrono::DateTime<chrono::Utc>) -> Plan {
    let mut remaining = plan.clone();
    let timeline = dpm_model::Timeline::at(plan, now);
    let completed = timeline.completed();
    remaining.dependencies = plan
        .enforced_dependencies()
        .filter(|d| !completed.contains(&d.successor))
        .filter_map(|d| {
            if !completed.contains(&d.predecessor) {
                return Some(d.clone());
            }
            let lag_hours = match timeline.edge(plan, d) {
                Release::Released { .. } | Release::AwaitingEvent => return None,
                Release::Elapsing { opens_at, .. } => {
                    (opens_at - now).num_milliseconds() as f64 / 3_600_000.0
                }
                Release::UnrecordedEventTime | Release::LagOutOfRange { .. } => d.lag_hours,
            };
            Some(Dependency {
                lag_hours,
                ..d.clone()
            })
        })
        .collect();
    remaining
}

/// Project a validated graph with an explicit finite, non-negative duration for every activity.
///
/// All relationships are lower bounds in elapsed hours; negative lag permits planned overlap.
pub fn deterministic_with_durations(
    plan: &Plan,
    durations: &Times,
) -> Result<Schedule, ScheduleError> {
    let order = topological_order(plan)?;
    plan.validate()?;
    for id in &order {
        let duration = durations
            .get(id)
            .ok_or(ScheduleError::InvalidDuration(*id))?;
        if !duration.is_finite() || *duration < 0.0 {
            return Err(ScheduleError::InvalidDuration(*id));
        }
    }
    let earliest = earliest_times(plan, durations, &order)?;
    let mut finish: f64 = 0.0;
    for id in &order {
        finish = finish.max(finite(*id, earliest[id] + durations[id])?);
    }
    let latest = latest_times(plan, durations, &order, finish)?;
    let mut activities = BTreeMap::new();
    let mut critical_activities = Vec::new();
    for id in order {
        let total_float = finite(id, latest[&id] - earliest[&id])?.max(0.0);
        let critical = total_float <= EPSILON;
        if critical {
            critical_activities.push(id);
        }
        activities.insert(
            id,
            ActivitySchedule {
                earliest_start_hours: earliest[&id],
                earliest_finish_hours: finite(id, earliest[&id] + durations[&id])?,
                latest_start_hours: latest[&id],
                latest_finish_hours: finite(id, latest[&id] + durations[&id])?,
                total_float_hours: total_float,
                free_float_hours: free_float(plan, durations, &earliest, id, finish)?
                    .min(total_float),
                critical,
            },
        );
    }
    Ok(Schedule {
        project_finish_hours: finish,
        activities,
        critical_activities,
    })
}

fn free_float(
    plan: &Plan,
    durations: &Times,
    earliest: &Times,
    id: WorkItemId,
    finish: f64,
) -> Result<f64, ScheduleError> {
    let mut available = finite(id, finish - earliest[&id] - durations[&id])?;
    for edge in plan.dependencies.iter().filter(|d| d.predecessor == id) {
        let weight = relation_weight(
            edge.kind,
            durations[&id],
            durations[&edge.successor],
            edge.lag_hours,
        );
        let slack = finite(id, earliest[&edge.successor] - earliest[&id] - weight)?;
        available = available.min(slack);
    }
    Ok(available.max(0.0))
}

fn earliest_times(
    plan: &Plan,
    durations: &Times,
    order: &[WorkItemId],
) -> Result<Times, ScheduleError> {
    let mut times: Times = order.iter().map(|id| (*id, 0.0)).collect();
    for successor in order {
        let mut start: f64 = 0.0;
        for dep in plan
            .dependencies
            .iter()
            .filter(|d| d.successor == *successor)
        {
            let weight = relation_weight(
                dep.kind,
                durations[&dep.predecessor],
                durations[successor],
                dep.lag_hours,
            );
            start = start.max(finite(*successor, times[&dep.predecessor] + weight)?);
        }
        times.insert(*successor, start);
    }
    Ok(times)
}

fn latest_times(
    plan: &Plan,
    durations: &Times,
    order: &[WorkItemId],
    finish: f64,
) -> Result<Times, ScheduleError> {
    let mut times: Times = order
        .iter()
        .map(|id| (*id, finish - durations[id]))
        .collect();
    for predecessor in order.iter().rev() {
        let mut bound = times[predecessor];
        for dep in plan
            .dependencies
            .iter()
            .filter(|d| d.predecessor == *predecessor)
        {
            let weight = relation_weight(
                dep.kind,
                durations[predecessor],
                durations[&dep.successor],
                dep.lag_hours,
            );
            bound = bound.min(finite(*predecessor, times[&dep.successor] - weight)?);
        }
        times.insert(*predecessor, bound);
    }
    Ok(times)
}

fn finite(work: WorkItemId, value: f64) -> Result<f64, ScheduleError> {
    if !value.is_finite() {
        return Err(ScheduleError::ArithmeticOverflow(work));
    }
    Ok(value)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
