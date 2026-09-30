use crate::network::{EPSILON, Network};
use crate::placement::Placement;
use crate::remaining_duration::RemainingDuration;
use crate::{ActivityCalendar, ActivitySchedule, RemainingOptions, Schedule, ScheduleError};
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
    deterministic_remaining_with(plan, timeline, RemainingOptions::default())
}

/// Remaining projection with caller-requested adjustments such as a review delay after the work
/// of every task still awaiting verification.
pub fn deterministic_remaining_with(
    plan: &Plan,
    timeline: &dpm_model::Timeline,
    options: RemainingOptions,
) -> Result<Schedule, ScheduleError> {
    options.validate()?;
    plan.validate()?;
    let remaining = remaining_plan_at(plan, timeline)?;
    let durations = remaining
        .durations()?
        .into_iter()
        .map(|(id, duration)| (id, duration.expected()))
        .collect();
    let network = Network::compile(&remaining.plan)?;
    let dense = dense(&network, &durations)?;
    let mut placement = remaining.placement(&network, timeline, &dense, options)?;
    let mut schedule = project(&network, &dense, placement.as_mut())?;
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
    /// Hours of each outstanding task's calendar from its recorded start event to the origin.
    pub(crate) elapsed: BTreeMap<WorkItemId, f64>,
    /// Applicable tasks whose verification is still outstanding.
    pub(crate) awaiting: BTreeSet<WorkItemId>,
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

    /// Calendar placement of this projection, or `None` for a plan without calendars and
    /// without a review delay. The initial window covers several times the total work, lag and
    /// review delay, so widening is rare.
    pub(crate) fn placement(
        &self,
        network: &Network,
        timeline: &dpm_model::Timeline,
        durations: &[f64],
        options: RemainingOptions,
    ) -> Result<Option<Placement>, ScheduleError> {
        let work: f64 = durations.iter().sum();
        let lags: f64 = self
            .plan
            .dependencies
            .iter()
            .map(|d| d.lag_hours.abs())
            .sum();
        let reviews = options.review_delay_hours * self.awaiting.len() as f64;
        Placement::new(
            &self.plan,
            network,
            timeline,
            &self.awaiting,
            5.0 * (work + lags + reviews),
            options,
        )
    }
}

/// Hours between each outstanding applicable task's recorded start and the origin: working hours
/// of its calendar when the plan has calendars, elapsed hours otherwise.
///
/// Blocked intervals count, since estimates are durations and not effort. A start whose time was
/// never recorded is absent, so that task keeps its whole duration rather than an assumed head
/// start.
fn elapsed_since_start(
    plan: &Plan,
    timeline: &dpm_model::Timeline,
    excluded: &BTreeSet<WorkItemId>,
) -> Result<BTreeMap<WorkItemId, f64>, ScheduleError> {
    plan.work_items
        .iter()
        .filter(|(id, work)| {
            work.is_executable()
                && !excluded.contains(id)
                && !work.execution.status.satisfies_dependency()
        })
        .filter_map(|(id, work)| match work.start_event()? {
            dpm_model::EventTime::Recorded(at) => Some(
                crate::placement::worked_between(plan, work, at, timeline.now())
                    .map(|hours| (*id, hours)),
            ),
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
pub(crate) fn remaining_plan_at(
    plan: &Plan,
    timeline: &dpm_model::Timeline,
) -> Result<Remaining, ScheduleError> {
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
            // A lag already elapsing ends at a fixed instant, measured in elapsed hours from now.
            let (lag_hours, lag_basis) = match timeline.edge(plan, d) {
                Release::Released { .. }
                | Release::SkippedBranch { .. }
                | Release::AwaitingEvent => return None,
                Release::Elapsing { opens_at, .. } => (
                    (opens_at - now).num_milliseconds() as f64 / 3_600_000.0,
                    dpm_model::LagBasis::Elapsed,
                ),
                Release::UnrecordedEventTime
                | Release::LagOutOfRange { .. }
                | Release::NotSelected => (d.lag_hours, d.lag_basis),
            };
            Some(Dependency {
                lag_hours,
                lag_basis,
                ..d.clone()
            })
        })
        .collect();
    let awaiting = plan
        .work_items
        .values()
        .filter(|w| w.is_executable() && !w.execution.status.satisfies_dependency())
        .filter(|w| !excluded.contains(&w.id))
        .map(|w| w.id)
        .collect();
    Ok(Remaining {
        awaiting,
        elapsed: elapsed_since_start(plan, timeline, &excluded)?,
        plan: remaining,
        excluded,
    })
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
    let dense = dense(&network, durations)?;
    project(&network, &dense, None)
}

/// Durations by topological position, each finite and non-negative.
fn dense(network: &Network, durations: &Times) -> Result<Vec<f64>, ScheduleError> {
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
    Ok(dense)
}

/// Project a compiled network, on calendars when a placement is given.
fn project(
    network: &Network,
    dense: &[f64],
    mut placement: Option<&mut Placement>,
) -> Result<Schedule, ScheduleError> {
    let times = match placement.as_deref_mut() {
        Some(placement) => network.times_placed(dense, placement)?,
        None => network.times(dense)?,
    };
    let placement = placement.as_deref();
    let mut activities = BTreeMap::new();
    let mut critical_activities = Vec::new();
    for (at, id) in network.order().iter().enumerate() {
        let read = |values: &[f64]| {
            values
                .get(at)
                .copied()
                .ok_or(ScheduleError::UnknownPosition(at))
        };
        let total_float = network.total_float(&times, at)?;
        let critical = total_float <= EPSILON;
        if critical {
            critical_activities.push(*id);
        }
        let (earliest_finish, latest_finish, review_wait) = times.finishes(network, dense, at)?;
        let calendar =
            placement
                .and_then(|p| p.resolved.get(id))
                .map(|resolved| ActivityCalendar {
                    resolved: resolved.clone(),
                    review_wait_hours: review_wait,
                });
        activities.insert(
            *id,
            ActivitySchedule {
                earliest_start_hours: read(&times.earliest)?,
                earliest_finish_hours: earliest_finish,
                latest_start_hours: read(&times.latest)?,
                latest_finish_hours: latest_finish,
                total_float_hours: total_float,
                free_float_hours: network
                    .free_float(dense, &times, at, placement)?
                    .min(total_float),
                critical,
                calendar,
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
