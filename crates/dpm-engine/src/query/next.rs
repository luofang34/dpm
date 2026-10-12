//! `next`: claimable work ranked by an explainable score, with the schedule risk of each candidate.

use super::{downstream_counts, explain, is_unestimated, simulation_config};
use crate::{EngineError, Transition, gates};
use chrono::{DateTime, Utc};
use dpm_model::{Plan, Priority, Timeline, WorkItem, WorkItemId};
use dpm_schedule::{deterministic_remaining_at, drag_remaining_at, simulate_remaining_at};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Which projection a candidate's criticality component comes from.
pub enum CriticalityBasis {
    /// The fraction of seeded simulations in which the task is critical.
    Simulated,
    /// Whether the task is on the deterministic critical path, as 1 or 0.
    Deterministic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Ready task with a reproducible priority score and its explanation.
pub struct NextWorkCandidate {
    /// Work-item projection represented by this result.
    pub work: WorkItem,
    /// Ranking score composed from criticality, priority, downstream work, and float.
    pub score: f64,
    /// Whether total float is within numerical tolerance of zero.
    pub critical: bool,
    /// Fraction of sampled schedules in which each activity is critical.
    pub criticality: Option<f64>,
    /// Projection the score's criticality component used.
    #[serde(default = "deterministic_basis")]
    pub criticality_basis: CriticalityBasis,
    /// Hours the deterministic finish would move if this task took no time: zero off the
    /// critical path, and negative when shortening it would delay the finish. Reported for
    /// context; the score does not use it.
    #[serde(default)]
    pub drag_hours: f64,
    /// Delay available without postponing project completion.
    pub total_float_hours: f64,
    /// Count of distinct transitively dependent work items.
    pub downstream_count: usize,
    /// Whether this work fits the requested capability set.
    pub capability_match: bool,
    /// Human-readable factors contributing to this recommendation.
    pub reasons: Vec<String>,
}

fn deterministic_basis() -> CriticalityBasis {
    CriticalityBasis::Deterministic
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Eligibility and scheduling inputs for a work recommendation.
pub struct NextWorkQuery {
    /// Required capabilities the caller provides; an empty set disables capability filtering.
    pub capabilities: BTreeSet<String>,
    /// Whether ranking should use sampled criticality instead of deterministic criticality.
    pub use_probabilistic_criticality: bool,
}

impl Default for NextWorkQuery {
    fn default() -> Self {
        Self {
            capabilities: BTreeSet::new(),
            use_probabilistic_criticality: true,
        }
    }
}

/// Recommend only tasks claimable at an adapter-supplied time and compatible with capabilities.
pub fn next_work(
    plan: &Plan,
    query: &NextWorkQuery,
    now: DateTime<Utc>,
) -> Result<Vec<NextWorkCandidate>, EngineError> {
    plan.validate()?;
    let timeline = Timeline::at(plan, now);
    let schedule = deterministic_remaining_at(plan, &timeline)?;
    let simulation = if query.use_probabilistic_criticality
        && plan
            .work_items
            .values()
            .any(|w| w.schedule.estimate.is_some())
    {
        Some(simulate_remaining_at(plan, simulation_config(), &timeline)?)
    } else {
        None
    };
    let reports = super::claim_reports(plan, &timeline);
    let eligible: BTreeSet<WorkItemId> = reports
        .iter()
        .filter(|(_, r)| r.ready)
        .filter_map(|(id, _)| plan.work_items.get(id))
        .filter(|w| {
            query.capabilities.is_empty() || w.contract.capabilities.is_subset(&query.capabilities)
        })
        .map(|w| w.id)
        .collect();
    let downstream = downstream_counts(plan, &timeline, eligible.iter().copied());
    // Only the candidates are recomputed, so drag never costs a pass per critical activity.
    let drags = drag_remaining_at(plan, &timeline, &eligible)?;

    let mut candidates = plan
        .work_items
        .values()
        .filter(|work| eligible.contains(&work.id))
        .filter_map(|work| reports.get(&work.id).map(|r| (work, r)))
        // Claimable work is applicable, so the active-graph schedule always covers it.
        .filter_map(|(work, claim)| {
            let finish = gates::evaluate(plan, work, Transition::Submit, &timeline);
            let activity = schedule.activities.get(&work.id)?;
            let mut candidate = candidate(
                work,
                query,
                (claim, &finish),
                activity,
                &downstream,
                simulation.as_ref(),
            );
            candidate.drag_hours = drags.get(&work.id).copied().unwrap_or(0.0);
            candidate.reasons.push(drag_reason(candidate.drag_hours));
            if is_unestimated(&timeline, work) {
                candidate.reasons.push(explain::UNESTIMATED_REASON.into());
            }
            Some(candidate)
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.work.key.natural_cmp(&b.work.key))
    });
    Ok(candidates)
}

/// The hours carry no space before their unit, so no drag statement reads as the unestimated
/// "0 h" statement.
fn drag_reason(hours: f64) -> String {
    format!(
        "drag {hours:.1}h is reported but not ranked: the score uses criticality, priority, downstream work and float"
    )
}

fn candidate(
    work: &WorkItem,
    query: &NextWorkQuery,
    gates: (&crate::GateReport, &crate::GateReport),
    activity: &dpm_schedule::ActivitySchedule,
    downstream: &BTreeMap<WorkItemId, usize>,
    simulation: Option<&dpm_schedule::SimulationSummary>,
) -> NextWorkCandidate {
    let downstream_count = downstream.get(&work.id).copied().unwrap_or(0);
    let criticality = simulation.and_then(|summary| summary.criticality.get(&work.id).copied());
    let capability_match = work.contract.capabilities.is_empty()
        || work.contract.capabilities.is_subset(&query.capabilities)
        || query.capabilities.is_empty();

    // Explicit components keep recommendation scores independently explainable.
    let critical_component = criticality.unwrap_or(if activity.critical { 1.0 } else { 0.0 });
    let capability_component = if capability_match { 15.0 } else { -100.0 };
    let priority_component = f64::from(work.schedule.priority.rank()) * 8.0;
    let downstream_component = libm::log(downstream_count as f64 + 1.0) * 6.0;
    let float_penalty = activity.total_float_hours.min(100.0) * 0.05;
    let score = critical_component * 100.0
        + capability_component
        + priority_component
        + downstream_component
        - float_penalty;

    NextWorkCandidate {
        work: work.clone(),
        score,
        critical: activity.critical,
        criticality,
        criticality_basis: if criticality.is_some() {
            CriticalityBasis::Simulated
        } else {
            CriticalityBasis::Deterministic
        },
        drag_hours: 0.0,
        total_float_hours: activity.total_float_hours,
        downstream_count,
        capability_match,
        reasons: reasons(
            work,
            gates,
            activity,
            criticality,
            downstream_count,
            capability_match,
        ),
    }
}

fn reasons(
    work: &WorkItem,
    (claim, finish): (&crate::GateReport, &crate::GateReport),
    activity: &dpm_schedule::ActivitySchedule,
    criticality: Option<f64>,
    downstream_count: usize,
    capability_match: bool,
) -> Vec<String> {
    let mut reasons = Vec::new();
    reasons.push(
        "start gates are satisfied: every FS/SS predecessor event and positive lag has elapsed"
            .into(),
    );
    reasons.extend(
        claim
            .provisional
            .iter()
            .map(crate::ProvisionalRelease::describe),
    );
    let pending_finish = finish
        .unmet
        .iter()
        .filter(|g| matches!(g, crate::UnmetGate::Dependency { .. }))
        .count();
    if pending_finish > 0 {
        reasons.push(format!(
            "submission additionally waits for {pending_finish} FF/SF prerequisite(s); see explain transitions.submit"
        ));
    }
    reasons.push(match criticality {
        Some(value) => format!(
            "criticality basis is simulated: critical in {:.0}% of schedule simulations",
            value * 100.0
        ),
        None if activity.critical => {
            "criticality basis is deterministic: on the deterministic critical path".into()
        }
        None => "criticality basis is deterministic: off the deterministic critical path".into(),
    });
    if downstream_count > 0 {
        reasons.push(format!("unblocks {downstream_count} downstream work items"));
    }
    if work.schedule.priority <= Priority::P1 {
        reasons.push(format!("human priority is {:?}", work.schedule.priority));
    }
    if !work.contract.capabilities.is_empty() {
        reasons.push(if capability_match {
            "requested capabilities match".into()
        } else {
            "requested capabilities do not fully match".into()
        });
    }
    reasons
}

#[cfg(test)]
mod tests;
