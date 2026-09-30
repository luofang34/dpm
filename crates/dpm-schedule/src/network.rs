//! Dependency network compiled once per projection: topological positions and adjacency lists.
//!
//! Every pass over the graph reads only the edges of the activity at hand, so one projection is
//! O(N + E) and a simulation compiles the network once for all of its iterations.

use crate::ScheduleError;
use dpm_model::{DependencyKind, Plan, WorkItemId};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

mod calendar;

/// Absolute tolerance, in hours, below which total float counts as zero.
///
/// Inputs are elapsed hours; at a million hours the f64 spacing is about 1e-10, so rounding from
/// sums of durations and lags stays at least two orders of magnitude below this bound while any
/// intentional slack (whole seconds and above) stays far above it.
pub(crate) const EPSILON: f64 = 1e-8;

/// One constraint seen from one of its endpoints; `other` is the position of the far endpoint.
#[derive(Debug, Clone, Copy)]
struct Edge {
    other: usize,
    kind: DependencyKind,
    lag: f64,
    /// The lag counts working hours of the successor's calendar.
    working: bool,
}

/// A validated plan's activities in topological order with their incoming and outgoing edges.
///
/// Edges keep the plan's dependency order, so every error names the same activity as a scan of
/// the dependency list would.
#[derive(Debug)]
pub(crate) struct Network {
    order: Vec<WorkItemId>,
    positions: BTreeMap<WorkItemId, usize>,
    incoming: Vec<Vec<Edge>>,
    outgoing: Vec<Vec<Edge>>,
}

/// Earliest and latest start and finish times by topological position, and the project finish.
///
/// A finish is the event successors wait for: with calendars, a task's verification, which may
/// wait for the verifier's working time after its execution ends. The elapsed pass leaves the
/// finish and review vectors empty, since a finish is then its start plus its duration and
/// simulations would otherwise allocate them for every sample.
pub(crate) struct Times {
    pub(crate) earliest: Vec<f64>,
    pub(crate) latest: Vec<f64>,
    pub(crate) earliest_finish: Vec<f64>,
    pub(crate) latest_finish: Vec<f64>,
    /// Hours between the end of execution and the verified finish at the earliest times.
    pub(crate) review_wait: Vec<f64>,
    pub(crate) finish: f64,
}

pub(crate) fn relation_weight(
    kind: DependencyKind,
    predecessor: f64,
    successor: f64,
    lag: f64,
) -> f64 {
    match kind {
        DependencyKind::FinishStart => predecessor + lag,
        DependencyKind::StartStart => lag,
        DependencyKind::FinishFinish => predecessor - successor + lag,
        DependencyKind::StartFinish => -successor + lag,
    }
}

pub(crate) fn finite(work: WorkItemId, value: f64) -> Result<f64, ScheduleError> {
    if !value.is_finite() {
        return Err(ScheduleError::ArithmeticOverflow(work));
    }
    Ok(value)
}

/// The duration at a topological position; durations built for another network are reported as
/// that activity's invalid duration.
fn at(values: &[f64], position: usize, id: WorkItemId) -> Result<f64, ScheduleError> {
    values
        .get(position)
        .copied()
        .ok_or(ScheduleError::InvalidDuration(id))
}

/// A projected time at a topological position; times always come from this network, so a miss is
/// an inconsistent projection.
fn time_at(values: &[f64], position: usize) -> Result<f64, ScheduleError> {
    values
        .get(position)
        .copied()
        .ok_or(ScheduleError::UnknownPosition(position))
}

impl Times {
    /// Earliest and latest finish and review wait of the activity at a position.
    pub(crate) fn finishes(
        &self,
        network: &Network,
        durations: &[f64],
        position: usize,
    ) -> Result<(f64, f64, f64), ScheduleError> {
        if self.earliest_finish.is_empty() {
            let id = network
                .order
                .get(position)
                .ok_or(ScheduleError::UnknownPosition(position))?;
            let duration = at(durations, position, *id)?;
            let earliest = finite(*id, time_at(&self.earliest, position)? + duration)?;
            let latest = finite(*id, time_at(&self.latest, position)? + duration)?;
            return Ok((earliest, latest, 0.0));
        }
        Ok((
            time_at(&self.earliest_finish, position)?,
            time_at(&self.latest_finish, position)?,
            time_at(&self.review_wait, position)?,
        ))
    }
}

impl Network {
    /// Order the activities topologically and validate the plan once.
    pub(crate) fn compile(plan: &Plan) -> Result<Self, ScheduleError> {
        let order = topological_order(plan)?;
        plan.validate()?;
        let positions: BTreeMap<_, _> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let mut incoming = vec![Vec::new(); order.len()];
        let mut outgoing = vec![Vec::new(); order.len()];
        for dep in &plan.dependencies {
            let position = |id: &WorkItemId| {
                positions
                    .get(id)
                    .copied()
                    .ok_or(ScheduleError::MissingWorkItem(*id))
            };
            let (from, to) = (position(&dep.predecessor)?, position(&dep.successor)?);
            let edge = |other| Edge {
                other,
                kind: dep.kind,
                lag: dep.lag_hours,
                working: dep.lag_basis == dpm_model::LagBasis::Working,
            };
            if let Some(edges) = outgoing.get_mut(from) {
                edges.push(edge(to));
            }
            if let Some(edges) = incoming.get_mut(to) {
                edges.push(edge(from));
            }
        }
        Ok(Self {
            order,
            positions,
            incoming,
            outgoing,
        })
    }

    /// Activities in topological order.
    pub(crate) fn order(&self) -> &[WorkItemId] {
        &self.order
    }

    /// Topological position of an activity of the compiled plan.
    pub(crate) fn position(&self, id: &WorkItemId) -> Option<usize> {
        self.positions.get(id).copied()
    }

    /// Forward and backward pass with one duration per topological position.
    pub(crate) fn times(&self, durations: &[f64]) -> Result<Times, ScheduleError> {
        // Incoming edges come from earlier positions, so each start is pushed after every value it
        // reads.
        let mut earliest = Vec::with_capacity(self.order.len());
        for ((position, id), edges) in self.order.iter().enumerate().zip(&self.incoming) {
            let duration = at(durations, position, *id)?;
            let mut start: f64 = 0.0;
            for edge in edges {
                let before = at(durations, edge.other, *id)?;
                let weight = relation_weight(edge.kind, before, duration, edge.lag);
                start = start.max(finite(*id, time_at(&earliest, edge.other)? + weight)?);
            }
            earliest.push(start);
        }
        // A separate pass keeps overflow reports in the order of the forward pass.
        let mut finish: f64 = 0.0;
        for ((id, start), duration) in self.order.iter().zip(&earliest).zip(durations) {
            finish = finish.max(finite(*id, start + duration)?);
        }
        let mut latest: Vec<f64> = durations.iter().map(|d| finish - d).collect();
        for ((position, id), edges) in self.order.iter().enumerate().zip(&self.outgoing).rev() {
            let duration = at(durations, position, *id)?;
            let mut bound = time_at(&latest, position)?;
            for edge in edges {
                let after = at(durations, edge.other, *id)?;
                let weight = relation_weight(edge.kind, duration, after, edge.lag);
                bound = bound.min(finite(*id, time_at(&latest, edge.other)? - weight)?);
            }
            if let Some(slot) = latest.get_mut(position) {
                *slot = bound;
            }
        }
        Ok(Times {
            earliest_finish: Vec::new(),
            latest_finish: Vec::new(),
            review_wait: Vec::new(),
            earliest,
            latest,
            finish,
        })
    }

    /// Slack before any successor constraint or the project finish would move.
    pub(crate) fn free_float(
        &self,
        durations: &[f64],
        times: &Times,
        position: usize,
        placement: Option<&crate::placement::Placement>,
    ) -> Result<f64, ScheduleError> {
        if let Some(placement) = placement {
            return self.free_float_placed(times, position, placement);
        }
        let (Some(id), Some(edges)) = (self.order.get(position), self.outgoing.get(position))
        else {
            return Err(ScheduleError::UnknownPosition(position));
        };
        let duration = at(durations, position, *id)?;
        let start = time_at(&times.earliest, position)?;
        let mut available = finite(*id, times.finish - start - duration)?;
        for edge in edges {
            let after = at(durations, edge.other, *id)?;
            let weight = relation_weight(edge.kind, duration, after, edge.lag);
            let slack = finite(*id, time_at(&times.earliest, edge.other)? - start - weight)?;
            available = available.min(slack);
        }
        Ok(available.max(0.0))
    }

    /// Total float, clamped at zero, of the activity at `position`.
    pub(crate) fn total_float(&self, times: &Times, position: usize) -> Result<f64, ScheduleError> {
        let Some(id) = self.order.get(position) else {
            return Err(ScheduleError::UnknownPosition(position));
        };
        let slack = time_at(&times.latest, position)? - time_at(&times.earliest, position)?;
        Ok(finite(*id, slack)?.max(0.0))
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
