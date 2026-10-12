//! Schedule drag: how much the deterministic project finish would move if one activity took no
//! time at all.
//!
//! Drag is measured by recomputing the projection with that activity's duration set to zero,
//! because with SS, FF and SF constraints and lags no closed form over float holds in general.
//! Only the requested activities are recomputed, one pass each, so a caller asking about a few
//! candidates never pays for every critical activity of a large plan.

use crate::network::{EPSILON, Network};
use crate::placement::Placement;
use crate::remaining_duration::RemainingDuration;
use crate::{RemainingOptions, ScheduleError};
use dpm_model::{Plan, WorkItemId};
use std::collections::{BTreeMap, BTreeSet};

/// Drag in hours of each requested activity in the full baseline projection.
///
/// An activity off the critical path has no drag. A negative drag marks an activity whose
/// shortening would delay the finish, as an FF or SF constraint can make happen.
pub fn drag(
    plan: &Plan,
    work: &BTreeSet<WorkItemId>,
) -> Result<BTreeMap<WorkItemId, f64>, ScheduleError> {
    plan.validate()?;
    let durations = plan
        .work_items
        .iter()
        .map(|(id, item)| Ok((*id, RemainingDuration::of(*id, item, false, 0.0)?)))
        .collect::<Result<Vec<_>, ScheduleError>>()?;
    let network = Network::compile(plan)?;
    let dense = dense(&network, &durations)?;
    recompute(&network, dense, None, work)
}

/// Drag in hours of each requested activity in the remaining projection
/// [`deterministic_remaining_at`](crate::deterministic_remaining_at) computes.
///
/// Work outside the active graph, and work the plan does not contain, is absent from the result.
pub fn drag_remaining_at(
    plan: &Plan,
    timeline: &dpm_model::Timeline,
    work: &BTreeSet<WorkItemId>,
) -> Result<BTreeMap<WorkItemId, f64>, ScheduleError> {
    plan.validate()?;
    let remaining = crate::cpm::remaining_plan_at(plan, timeline)?;
    let durations = remaining.durations()?;
    let network = Network::compile(&remaining.plan)?;
    let dense = dense(&network, &durations)?;
    let mut placement =
        remaining.placement(&network, timeline, &dense, RemainingOptions::default())?;
    let requested = work
        .iter()
        .filter(|id| !remaining.excluded.contains(id))
        .copied()
        .collect();
    recompute(&network, dense, placement.as_mut(), &requested)
}

/// Expected durations by topological position.
fn dense(
    network: &Network,
    durations: &[(WorkItemId, RemainingDuration)],
) -> Result<Vec<f64>, ScheduleError> {
    let mut dense = vec![0.0; network.order().len()];
    for (id, source) in durations {
        let at = network
            .position(id)
            .ok_or(ScheduleError::MissingWorkItem(*id))?;
        if let Some(slot) = dense.get_mut(at) {
            *slot = source.expected();
        }
    }
    Ok(dense)
}

fn recompute(
    network: &Network,
    mut durations: Vec<f64>,
    mut placement: Option<&mut Placement>,
    work: &BTreeSet<WorkItemId>,
) -> Result<BTreeMap<WorkItemId, f64>, ScheduleError> {
    let mut times = |durations: &[f64]| match placement.as_deref_mut() {
        Some(placement) => network.times_placed(durations, placement),
        None => network.times(durations),
    };
    let base = times(&durations)?;
    let mut drags = BTreeMap::new();
    for id in work {
        let Some(at) = network.position(id) else {
            continue;
        };
        if network.total_float(&base, at)? > EPSILON {
            drags.insert(*id, 0.0);
            continue;
        }
        let Some(slot) = durations.get_mut(at) else {
            continue;
        };
        let duration = std::mem::replace(slot, 0.0);
        let shortened = times(&durations)?;
        if let Some(slot) = durations.get_mut(at) {
            *slot = duration;
        }
        let drag = base.finish - shortened.finish;
        // Rounding in the two passes must not report a drag the plan does not have.
        drags.insert(*id, if drag.abs() <= EPSILON { 0.0 } else { drag });
    }
    Ok(drags)
}

#[cfg(test)]
mod tests;
