use crate::{ActivitySchedule, Schedule, ScheduleError};
use dpm_model::{DependencyKind, Plan, WorkItemId};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
pub fn deterministic_remaining(plan: &Plan) -> Result<Schedule, ScheduleError> {
    plan.validate()?;
    let remaining = remaining_plan(plan);
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

pub(crate) fn remaining_plan(plan: &Plan) -> Plan {
    let mut remaining = plan.clone();
    remaining.dependencies.retain(|d| {
        !plan.work_items[&d.predecessor]
            .status
            .satisfies_dependency()
            && !plan.work_items[&d.successor].status.satisfies_dependency()
    });
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
