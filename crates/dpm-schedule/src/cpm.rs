use crate::network::{EPSILON, Network, finite};
use crate::remaining_duration::RemainingDuration;
use crate::{ActivitySchedule, Schedule, ScheduleError};
use dpm_model::{Dependency, Plan, Release, WorkItemId};
use std::collections::{BTreeMap, BTreeSet};

type Times = BTreeMap<WorkItemId, f64>;

/// Project the full baseline using each task's weighted PERT duration.
pub fn deterministic(plan: &Plan) -> Result<Schedule, ScheduleError> {
    let durations = plan
        .work_items
        .iter()
        .map(|(id, work)| (*id, work.expected_duration_hours()))
        .collect();
    deterministic_with_durations(plan, &durations)
}

/// Project outstanding applicable work, treating verified tasks as zero remaining duration.
///
/// A task with a recorded start event contributes the mean of its remaining duration conditional
/// on still running after the hours elapsed since that start, the same distribution the remaining
/// simulation samples. Waived constraints no longer bound outstanding work; the baseline
/// projection keeps them. Work that is not applicable (not selected, undecided, awaiting a
/// choice, stranded or an empty join) is absent from the result, so the projection covers the
/// same active graph as execution gates. `now` is the adapter's clock reading, used to decide
/// which milestones are reached and how long started work has run.
pub fn deterministic_remaining(
    plan: &Plan,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Schedule, ScheduleError> {
    deterministic_remaining_at(plan, &dpm_model::Timeline::at(plan, now))
}

/// Remaining projection sharing the gate timeline for the same immutable plan snapshot.
pub fn deterministic_remaining_at(
    plan: &Plan,
    timeline: &dpm_model::Timeline,
) -> Result<Schedule, ScheduleError> {
    plan.validate()?;
    let remaining = remaining_plan_at(plan, timeline);
    let durations = remaining
        .durations()?
        .into_iter()
        .map(|(id, duration)| (id, duration.expected()))
        .collect();
    let mut schedule = deterministic_with_durations(&remaining.plan, &durations)?;
    schedule
        .activities
        .retain(|id, _| !remaining.excluded.contains(id));
    schedule
        .critical_activities
        .retain(|id| !remaining.excluded.contains(id));
    Ok(schedule)
}

/// Outstanding constraints of the active graph, and the work excluded from it.
pub(crate) struct Remaining {
    /// Plan whose excluded work has no duration and no constraints.
    pub(crate) plan: Plan,
    /// Work that is not applicable and must not appear in any remaining projection.
    pub(crate) excluded: BTreeSet<WorkItemId>,
    /// Hours from each outstanding task's recorded start event to the origin.
    pub(crate) elapsed: BTreeMap<WorkItemId, f64>,
}

impl Remaining {
    /// Remaining duration source of every activity, in work-item id order.
    pub(crate) fn durations(&self) -> Result<Vec<(WorkItemId, RemainingDuration)>, ScheduleError> {
        self.plan
            .work_items
            .iter()
            .map(|(id, work)| {
                let done = work.execution.status.satisfies_dependency();
                let elapsed = self.elapsed.get(id).copied().unwrap_or(0.0);
                Ok((*id, RemainingDuration::of(*id, work, done, elapsed)?))
            })
            .collect()
    }
}

/// Hours between each outstanding applicable task's recorded start and the origin.
///
/// Blocked intervals count, since estimates are elapsed hours. A start whose time was never
/// recorded is absent, so that task keeps its whole duration rather than an assumed head start.
fn elapsed_since_start(
    plan: &Plan,
    timeline: &dpm_model::Timeline,
    excluded: &BTreeSet<WorkItemId>,
) -> BTreeMap<WorkItemId, f64> {
    plan.work_items
        .iter()
        .filter(|(id, work)| {
            work.is_executable()
                && !excluded.contains(id)
                && !work.execution.status.satisfies_dependency()
        })
        .filter_map(|(id, work)| match work.start_event()? {
            dpm_model::EventTime::Recorded(at) => Some((
                *id,
                ((timeline.now() - at).num_milliseconds() as f64 / 3_600_000.0).max(0.0),
            )),
            dpm_model::EventTime::Unrecorded => None,
        })
        .collect()
}

/// Outstanding constraints measured from `now` as the projection origin.
///
/// Edges into completed work and waived edges no longer bound anything. An edge from completed
/// work keeps only the part of its lag the shared gate evaluator still reports as elapsing, so the
/// forecast never releases a constraint before execution does; a lag whose event time was never recorded is
/// kept whole rather than assumed to have elapsed. The completed predecessor projects at the origin
/// with zero duration, so the kept lag is measured from `now`. A start-based edge from work that has
/// started is treated the same way from its start event; the started predecessor projects at the
/// origin unless its own remaining constraints push it later, which only delays the forecast, and
/// runs for its remaining duration from there. Work the shared evaluator reports
/// as not applicable keeps no duration and no edge, so it cannot move applicable work.
pub(crate) fn remaining_plan_at(plan: &Plan, timeline: &dpm_model::Timeline) -> Remaining {
    let mut remaining = plan.clone();
    let now = timeline.now();
    let completed = timeline.completed();
    let excluded: BTreeSet<_> = plan
        .work_items
        .keys()
        .filter(|id| !timeline.applicability(**id).is_applicable())
        .copied()
        .collect();
    for id in &excluded {
        if let Some(work) = remaining.work_items.get_mut(id) {
            work.schedule.estimate = None;
        }
    }
    remaining.dependencies = plan
        .enforced_dependencies()
        .filter(|d| !excluded.contains(&d.predecessor) && !excluded.contains(&d.successor))
        .filter(|d| !completed.contains(&d.successor))
        .filter_map(|d| {
            if !completed.contains(&d.predecessor) && !started_from(plan, timeline, d) {
                return Some(d.clone());
            }
            let lag_hours = match timeline.edge(plan, d) {
                Release::Released { .. }
                | Release::SkippedBranch { .. }
                | Release::AwaitingEvent => return None,
                Release::Elapsing { opens_at, .. } => {
                    (opens_at - now).num_milliseconds() as f64 / 3_600_000.0
                }
                Release::UnrecordedEventTime
                | Release::LagOutOfRange { .. }
                | Release::NotSelected => d.lag_hours,
            };
            Some(Dependency {
                lag_hours,
                ..d.clone()
            })
        })
        .collect();
    Remaining {
        elapsed: elapsed_since_start(plan, timeline, &excluded),
        plan: remaining,
        excluded,
    }
}

/// Whether a start-based edge (SS, SF) waits on a predecessor start that has already occurred.
///
/// Its lag then elapses from that start exactly as the gate evaluator measures it, rather than
/// from the predecessor's projected start at the origin. Finish-based edges still wait for
/// verification, so a submitted or provisionally relied-on predecessor keeps its whole edge.
fn started_from(plan: &Plan, timeline: &dpm_model::Timeline, edge: &Dependency) -> bool {
    edge.kind.predecessor_endpoint() == dpm_model::Endpoint::Start
        && timeline
            .event(plan, edge.predecessor, dpm_model::Endpoint::Start)
            .is_some()
}

/// Project a validated graph with an explicit finite, non-negative duration for every activity.
///
/// All relationships are lower bounds in elapsed hours; negative lag permits planned overlap.
pub fn deterministic_with_durations(
    plan: &Plan,
    durations: &Times,
) -> Result<Schedule, ScheduleError> {
    let network = Network::compile(plan)?;
    let mut dense = Vec::with_capacity(network.order().len());
    for id in network.order() {
        let duration = durations
            .get(id)
            .ok_or(ScheduleError::InvalidDuration(*id))?;
        if !duration.is_finite() || *duration < 0.0 {
            return Err(ScheduleError::InvalidDuration(*id));
        }
        dense.push(*duration);
    }
    let times = network.times(&dense)?;
    let mut activities = BTreeMap::new();
    let mut critical_activities = Vec::new();
    let spans = times.earliest.iter().zip(&times.latest).zip(&dense);
    for ((at, id), ((earliest, latest), duration)) in network.order().iter().enumerate().zip(spans)
    {
        let total_float = network.total_float(&times, at)?;
        let critical = total_float <= EPSILON;
        if critical {
            critical_activities.push(*id);
        }
        activities.insert(
            *id,
            ActivitySchedule {
                earliest_start_hours: *earliest,
                earliest_finish_hours: finite(*id, earliest + duration)?,
                latest_start_hours: *latest,
                latest_finish_hours: finite(*id, latest + duration)?,
                total_float_hours: total_float,
                free_float_hours: network.free_float(&dense, &times, at)?.min(total_float),
                critical,
            },
        );
    }
    Ok(Schedule {
        project_finish_hours: times.finish,
        activities,
        critical_activities,
    })
}

#[cfg(test)]
mod tests;
