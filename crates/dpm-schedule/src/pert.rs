//! Classical analytic PERT approximation of the project finish along one mean critical path.
//!
//! PERT treats the finish as normal, with the mean and variance of a single critical path of the
//! deterministic projection. Any near-critical path that overtakes it in some outcomes is ignored,
//! so where paths merge the approximation usually overstates the probability of finishing by a
//! horizon. It is reported beside the sampled probability, never in place of it.

use crate::network::{EPSILON, Network, Times};
use crate::placement::Placement;
use crate::remaining_duration::RemainingDuration;
use crate::{RemainingOptions, ScheduleError};
use dpm_model::{DependencyId, Endpoint, Plan, ThreePointEstimate, WorkItemId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// What the approximation leaves out, stated wherever its probability is reported.
pub const PERT_APPROXIMATION_NOTE: &str = "approximation: classical PERT normal over one mean critical path; it ignores near-critical paths and therefore usually overstates the probability of finishing by the horizon";

/// Classical PERT variance `((P − O) / 6)²` of a three-point estimate, in hours squared.
///
/// It is not the variance of the beta-PERT distribution the simulation samples, whose variance
/// depends on where the most-likely value lies.
#[must_use]
pub fn pert_variance_hours2(estimate: ThreePointEstimate) -> f64 {
    let spread = (estimate.pessimistic_hours - estimate.optimistic_hours).max(0.0) / 6.0;
    spread * spread
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Normal approximation of the project finish along one mean critical path.
pub struct PertApproximation {
    /// Deterministic project finish in elapsed hours; without calendars it is the sum of the
    /// path's PERT means and lags.
    pub mean_hours: f64,
    /// Sum of the classical PERT variances of the durations the path's finish depends on. On a
    /// calendar these are working hours, not the elapsed hours of `mean_hours`, so the spread of
    /// the elapsed finish is understated and the approximation is more confident still.
    pub variance_hours2: f64,
    /// Square root of `variance_hours2`.
    pub sigma_hours: f64,
    /// The mean critical path from its first activity to the one that sets the project finish.
    pub path: Vec<WorkItemId>,
    /// Classical PERT variance of every projected activity's remaining duration: zero for
    /// finished or exactly known work, and for started work the range its elapsed time allows.
    pub activity_variance_hours2: BTreeMap<WorkItemId, f64>,
}

impl PertApproximation {
    /// `Φ((H − mean) / σ)`; a path without variance finishes by `H` exactly when its mean does.
    #[must_use]
    pub fn probability_by(&self, horizon_hours: f64) -> f64 {
        if self.sigma_hours <= EPSILON {
            return if horizon_hours + EPSILON >= self.mean_hours {
                1.0
            } else {
                0.0
            };
        }
        let z = (horizon_hours - self.mean_hours) / self.sigma_hours;
        0.5 * (1.0 + libm::erf(z / std::f64::consts::SQRT_2))
    }
}

/// The approximation of the full baseline, every task at its whole PERT duration.
pub fn pert_approximation(plan: &Plan) -> Result<PertApproximation, ScheduleError> {
    plan.validate()?;
    let durations = plan
        .work_items
        .iter()
        .map(|(id, work)| Ok((*id, RemainingDuration::of(*id, work, false, 0.0)?)))
        .collect::<Result<Vec<_>, ScheduleError>>()?;
    let network = Network::compile(plan)?;
    approximate(&network, &durations, None)
}

/// The approximation of the remaining graph that
/// [`deterministic_remaining_with`](crate::deterministic_remaining_with) projects.
///
/// Started work contributes its remaining mean and the variance of the range still possible;
/// finished work contributes neither. Excluded work is on no path and has no variance.
pub fn pert_approximation_remaining_with(
    plan: &Plan,
    timeline: &dpm_model::Timeline,
    options: RemainingOptions,
) -> Result<PertApproximation, ScheduleError> {
    options.validate()?;
    plan.validate()?;
    let remaining = crate::cpm::remaining_plan_at(plan, timeline)?;
    let durations = remaining.durations()?;
    let network = Network::compile(&remaining.plan)?;
    let expected: Vec<f64> = durations.iter().map(|(_, d)| d.expected()).collect();
    let mut placement = remaining.placement(&network, timeline, &expected, options)?;
    let mut approximation = approximate(&network, &durations, placement.as_mut())?;
    approximation
        .activity_variance_hours2
        .retain(|id, _| !remaining.excluded.contains(id));
    approximation
        .path
        .retain(|id| !remaining.excluded.contains(id));
    Ok(approximation)
}

/// [`pert_approximation_remaining_with`] without adjustments.
pub fn pert_approximation_remaining_at(
    plan: &Plan,
    timeline: &dpm_model::Timeline,
) -> Result<PertApproximation, ScheduleError> {
    pert_approximation_remaining_with(plan, timeline, RemainingOptions::default())
}

fn approximate(
    network: &Network,
    durations_by_id: &[(WorkItemId, RemainingDuration)],
    mut placement: Option<&mut Placement>,
) -> Result<PertApproximation, ScheduleError> {
    let count = network.order().len();
    let (mut means, mut variances) = (vec![0.0; count], vec![0.0; count]);
    for (id, source) in durations_by_id {
        let at = network
            .position(id)
            .ok_or(ScheduleError::MissingWorkItem(*id))?;
        if let (Some(mean), Some(variance)) = (means.get_mut(at), variances.get_mut(at)) {
            *mean = source.expected();
            *variance = source.pert_variance();
        }
    }
    let times = match placement.as_deref_mut() {
        Some(placement) => network.times_placed(&means, placement)?,
        None => network.times(&means)?,
    };
    let critical = (0..count)
        .map(|at| Ok(network.total_float(&times, at)? <= EPSILON))
        .collect::<Result<Vec<bool>, ScheduleError>>()?;
    let critical_at = |at: usize| critical.get(at).copied().unwrap_or(false);
    let critical_links: Vec<_> = network
        .relation_slacks(&means, &times, placement.as_deref())?
        .into_iter()
        .filter(|(_, from, to, slack)| *slack <= EPSILON && critical_at(*from) && critical_at(*to))
        .collect();
    let links = Links {
        ids: critical_links.iter().map(|(id, ..)| *id).collect(),
        continued: critical_links.iter().map(|(_, from, ..)| *from).collect(),
    };
    let (path, variance) = mean_critical_path(network, &times, &means, &variances, &links)?;
    Ok(PertApproximation {
        mean_hours: times.finish,
        variance_hours2: variance,
        sigma_hours: variance.sqrt(),
        path,
        activity_variance_hours2: network.order().iter().copied().zip(variances).collect(),
    })
}

/// Critical driving constraints, and the positions a critical path continues from.
struct Links {
    ids: BTreeSet<DependencyId>,
    continued: BTreeSet<usize>,
}

/// Variance of an activity's start and finish events along the best path reaching it, and the
/// predecessor that path arrives from.
#[derive(Debug, Clone, Copy)]
struct Reach {
    start: f64,
    finish: f64,
    from: Option<usize>,
}

/// One deterministic mean critical path and the variance of its finish.
///
/// Each activity is reached over critical driving constraints; where several reach it, the one
/// that gives its finish the larger variance wins, ties going to the smaller predecessor id. A
/// constraint fixes one event of its successor from one event of its predecessor, and the other
/// event of the successor lies one duration away, so every duration on the path counts once.
/// The path ends at a critical activity that sets the project finish and from which no critical
/// constraint continues, such as a closing milestone, chosen the same way.
fn mean_critical_path(
    network: &Network,
    times: &Times,
    means: &[f64],
    variances: &[f64],
    links: &Links,
) -> Result<(Vec<WorkItemId>, f64), ScheduleError> {
    let order = network.order();
    let id_at = |at: usize| order.get(at).copied();
    let mut reach: Vec<Reach> = Vec::with_capacity(order.len());
    for at in 0..order.len() {
        let own = variances.get(at).copied().unwrap_or(0.0);
        let mut best: Option<(f64, usize, Endpoint, f64)> = None;
        for edge in network
            .incoming(at)
            .iter()
            .filter(|e| links.ids.contains(&e.id))
        {
            let Some(before) = reach.get(edge.other) else {
                continue;
            };
            let carried = match edge.kind.predecessor_endpoint() {
                Endpoint::Start => before.start,
                Endpoint::Finish => before.finish,
            };
            // Links are compared by the variance they give this activity's finish: a link fixing
            // its start adds its own duration, one fixing its finish does not.
            let produced = match edge.kind.successor_endpoint() {
                Endpoint::Start => carried + own,
                Endpoint::Finish => carried,
            };
            let better = best.is_none_or(|(_, other, _, finish)| {
                prefers((produced, id_at(edge.other)), (finish, id_at(other)))
            });
            if better {
                best = Some((
                    carried,
                    edge.other,
                    edge.kind.successor_endpoint(),
                    produced,
                ));
            }
        }
        reach.push(match best {
            None => Reach {
                start: 0.0,
                finish: own,
                from: None,
            },
            Some((carried, from, Endpoint::Start, _)) => Reach {
                start: carried,
                finish: carried + own,
                from: Some(from),
            },
            Some((carried, from, Endpoint::Finish, _)) => Reach {
                start: carried + own,
                finish: carried,
                from: Some(from),
            },
        });
    }
    let mut end: Option<(f64, usize)> = None;
    for (at, reached) in reach.iter().enumerate() {
        let finish = times.finishes(network, means, at)?.0;
        let critical = network.total_float(times, at)? <= EPSILON;
        if critical
            && !links.continued.contains(&at)
            && finish + EPSILON >= times.finish
            && end.is_none_or(|(variance, other)| {
                prefers((reached.finish, id_at(at)), (variance, id_at(other)))
            })
        {
            end = Some((reached.finish, at));
        }
    }
    let Some((variance, mut at)) = end else {
        return Ok((Vec::new(), 0.0));
    };
    let mut path = Vec::new();
    // Each step moves to an earlier topological position, so the walk ends within the network.
    while let Some(id) = id_at(at) {
        path.push(id);
        match reach.get(at).and_then(|r| r.from) {
            Some(from) => at = from,
            None => break,
        }
    }
    path.reverse();
    Ok((path, variance))
}

/// Whether a branch with `candidate`'s variance and work id wins over `current`'s: the larger
/// variance, then the smaller id, so the choice never depends on traversal order.
fn prefers(candidate: (f64, Option<WorkItemId>), current: (f64, Option<WorkItemId>)) -> bool {
    match candidate.0.total_cmp(&current.0) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Equal => candidate.1 < current.1,
        std::cmp::Ordering::Less => false,
    }
}

#[cfg(test)]
mod tests;
