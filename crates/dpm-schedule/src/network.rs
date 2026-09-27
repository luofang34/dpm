//! Dependency network compiled once per projection: topological positions and adjacency lists.
//!
//! Every pass over the graph reads only the edges of the activity at hand, so one projection is
//! O(N + E) and a simulation compiles the network once for all of its iterations.

use crate::ScheduleError;
use dpm_model::{DependencyKind, Plan, WorkItemId};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

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

/// Earliest and latest start times by topological position, and the project finish.
pub(crate) struct Times {
    pub(crate) earliest: Vec<f64>,
    pub(crate) latest: Vec<f64>,
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

impl Network {
    /// Order the activities topologically and validate the plan once.
    pub(crate) fn compile(plan: &Plan) -> Result<Self, ScheduleError> {
        let order = topological_order(plan)?;
        plan.validate()?;
        let positions: BTreeMap<_, _> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let mut incoming = vec![Vec::new(); order.len()];
        let mut outgoing = vec![Vec::new(); order.len()];
        for dep in &plan.dependencies {
            let from = positions[&dep.predecessor];
            let to = positions[&dep.successor];
            outgoing[from].push(Edge {
                other: to,
                kind: dep.kind,
                lag: dep.lag_hours,
            });
            incoming[to].push(Edge {
                other: from,
                kind: dep.kind,
                lag: dep.lag_hours,
            });
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
        let mut earliest = vec![0.0; self.order.len()];
        for (at, id) in self.order.iter().enumerate() {
            let mut start: f64 = 0.0;
            for edge in &self.incoming[at] {
                let weight =
                    relation_weight(edge.kind, durations[edge.other], durations[at], edge.lag);
                start = start.max(finite(*id, earliest[edge.other] + weight)?);
            }
            earliest[at] = start;
        }
        let mut finish: f64 = 0.0;
        for (at, id) in self.order.iter().enumerate() {
            finish = finish.max(finite(*id, earliest[at] + durations[at])?);
        }
        let mut latest: Vec<f64> = durations.iter().map(|d| finish - d).collect();
        for (at, id) in self.order.iter().enumerate().rev() {
            let mut bound = latest[at];
            for edge in &self.outgoing[at] {
                let weight =
                    relation_weight(edge.kind, durations[at], durations[edge.other], edge.lag);
                bound = bound.min(finite(*id, latest[edge.other] - weight)?);
            }
            latest[at] = bound;
        }
        Ok(Times {
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
        at: usize,
    ) -> Result<f64, ScheduleError> {
        let id = self.order[at];
        let mut available = finite(id, times.finish - times.earliest[at] - durations[at])?;
        for edge in &self.outgoing[at] {
            let weight = relation_weight(edge.kind, durations[at], durations[edge.other], edge.lag);
            let slack = finite(id, times.earliest[edge.other] - times.earliest[at] - weight)?;
            available = available.min(slack);
        }
        Ok(available.max(0.0))
    }

    /// Total float, clamped at zero, of the activity at `at`.
    pub(crate) fn total_float(&self, times: &Times, at: usize) -> Result<f64, ScheduleError> {
        Ok(finite(self.order[at], times.latest[at] - times.earliest[at])?.max(0.0))
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
