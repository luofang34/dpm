use crate::{EngineError, Transition, gates};
use chrono::{DateTime, Utc};
use dpm_model::{
    BasisStatus, DecisionStatus, Plan, Priority, Timeline, WorkItem, WorkItemId, WorkStatus,
    basis_status,
};
use dpm_schedule::{SimulationConfig, deterministic_remaining, simulate_remaining};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

mod explain;
pub use explain::{BasisReport, WorkExplanation, explain_work};
#[derive(Debug, Clone, Serialize, Deserialize)]
/// Execution counts and remaining schedule projections.
pub struct StatusSummary {
    /// Claim eligibility of each leaf task, using the same policy as commands and next.
    pub gates: BTreeMap<WorkItemId, crate::GateReport>,
    /// Reported execution progress, separate from verified completion.
    pub progress: crate::ProgressSummary,
    /// Authoritative wrapping revision used for this projection.
    pub revision: u64,
    /// Count of all tasks, milestones, and containers.
    pub total_work: usize,
    /// Number of tasks that can be claimed now.
    pub ready: usize,
    /// Number of explicitly blocked tasks.
    pub blocked: usize,
    /// Number of claimed or in-progress tasks.
    pub in_flight: usize,
    /// Number of tasks awaiting independent verification.
    pub awaiting_verification: usize,
    /// Number of tasks whose enforced provisional basis rests on a rejected predecessor attempt.
    #[serde(default)]
    pub basis_invalidated: usize,
    /// Count of completed tasks and derived aggregate completions.
    pub complete: usize,
    /// Number of unresolved decisions, whether contextual or blocking.
    pub open_decisions: usize,
    /// Deterministic remaining project duration in elapsed hours.
    pub expected_finish_hours: f64,
    /// Median sampled completion time in elapsed hours.
    pub p50_finish_hours: Option<f64>,
    /// 80th percentile sampled completion time in elapsed hours.
    pub p80_finish_hours: Option<f64>,
    /// 95th percentile sampled completion time in elapsed hours.
    pub p95_finish_hours: Option<f64>,
}

pub(crate) fn simulation_config() -> SimulationConfig {
    SimulationConfig {
        iterations: 2_000,
        ..SimulationConfig::default()
    }
}

/// Claim readiness of every leaf task, evaluated once for one timeline.
fn claim_reports(plan: &Plan, timeline: &Timeline) -> BTreeMap<WorkItemId, crate::GateReport> {
    plan.work_items
        .values()
        .filter(|w| w.is_executable())
        .map(|w| (w.id, gates::evaluate(plan, w, Transition::Claim, timeline)))
        .collect()
}

/// Project validated lifecycle counts and remaining duration at an adapter-supplied time.
pub fn status(
    plan: &Plan,
    probabilistic: bool,
    now: DateTime<Utc>,
) -> Result<StatusSummary, EngineError> {
    plan.validate()?;
    let timeline = Timeline::at(plan, now);
    let schedule = deterministic_remaining(plan, now)?;
    let simulation = if probabilistic && plan.work_items.values().any(|w| w.estimate.is_some()) {
        Some(simulate_remaining(plan, simulation_config(), now)?)
    } else {
        None
    };
    let gates = claim_reports(plan, &timeline);

    Ok(StatusSummary {
        ready: gates.values().filter(|report| report.ready).count(),
        gates,
        progress: crate::progress::progress_with(plan, &timeline).overall,
        revision: plan.revision,
        total_work: plan.work_items.len(),
        blocked: plan
            .work_items
            .values()
            .filter(|w| w.status == WorkStatus::Blocked)
            .count(),
        in_flight: plan
            .work_items
            .values()
            .filter(|w| matches!(w.status, WorkStatus::Claimed | WorkStatus::InProgress))
            .count(),
        awaiting_verification: plan
            .work_items
            .values()
            .filter(|w| w.status == WorkStatus::Submitted)
            .count(),
        basis_invalidated: plan
            .work_items
            .values()
            .filter(|w| basis_status(plan, w).iter().any(BasisStatus::gates))
            .count(),
        complete: timeline.completed().len(),
        open_decisions: plan
            .decisions
            .values()
            .filter(|decision| decision.status == DecisionStatus::Open)
            .count(),
        expected_finish_hours: schedule.project_finish_hours,
        p50_finish_hours: simulation.as_ref().map(|s| s.p50_finish_hours),
        p80_finish_hours: simulation.as_ref().map(|s| s.p80_finish_hours),
        p95_finish_hours: simulation.as_ref().map(|s| s.p95_finish_hours),
    })
}

fn downstream_counts(plan: &Plan) -> BTreeMap<WorkItemId, usize> {
    let mut outgoing = BTreeMap::<WorkItemId, Vec<WorkItemId>>::new();
    for dep in &plan.dependencies {
        outgoing
            .entry(dep.predecessor)
            .or_default()
            .push(dep.successor);
    }

    let mut result = BTreeMap::new();
    for start in plan.work_items.keys() {
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([*start]);
        while let Some(node) = queue.pop_front() {
            if let Some(next) = outgoing.get(&node) {
                for child in next {
                    if seen.insert(*child) {
                        queue.push_back(*child);
                    }
                }
            }
        }
        result.insert(*start, seen.len());
    }
    result
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
    /// Delay available without postponing project completion.
    pub total_float_hours: f64,
    /// Count of distinct transitively dependent work items.
    pub downstream_count: usize,
    /// Whether this work fits the requested capability set.
    pub capability_match: bool,
    /// Human-readable factors contributing to this recommendation.
    pub reasons: Vec<String>,
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
    let schedule = deterministic_remaining(plan, now)?;
    let downstream = downstream_counts(plan);
    let simulation = if query.use_probabilistic_criticality
        && plan.work_items.values().any(|w| w.estimate.is_some())
    {
        Some(simulate_remaining(plan, simulation_config(), now)?)
    } else {
        None
    };
    let reports = claim_reports(plan, &timeline);

    let mut candidates = plan
        .work_items
        .values()
        .filter_map(|work| reports.get(&work.id).filter(|r| r.ready).map(|r| (work, r)))
        .filter(|(work, _)| {
            query.capabilities.is_empty() || work.capabilities.is_subset(&query.capabilities)
        })
        .map(|(work, claim)| {
            let finish = gates::evaluate(plan, work, Transition::Submit, &timeline);
            candidate(
                work,
                query,
                (claim, &finish),
                &schedule.activities[&work.id],
                &downstream,
                simulation.as_ref(),
            )
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.work.key.cmp(&b.work.key))
    });
    Ok(candidates)
}

fn candidate(
    work: &WorkItem,
    query: &NextWorkQuery,
    (claim, finish): (&crate::GateReport, &crate::GateReport),
    activity: &dpm_schedule::ActivitySchedule,
    downstream: &BTreeMap<WorkItemId, usize>,
    simulation: Option<&dpm_schedule::SimulationSummary>,
) -> NextWorkCandidate {
    let downstream_count = downstream.get(&work.id).copied().unwrap_or(0);
    let criticality = simulation.and_then(|summary| summary.criticality.get(&work.id).copied());
    let capability_match = work.capabilities.is_empty()
        || work.capabilities.is_subset(&query.capabilities)
        || query.capabilities.is_empty();

    // Explicit components keep recommendation scores independently explainable.
    let critical_component = criticality.unwrap_or(if activity.critical { 1.0 } else { 0.0 });
    let capability_component = if capability_match { 15.0 } else { -100.0 };
    let priority_component = f64::from(work.priority.rank()) * 8.0;
    let downstream_component = (downstream_count as f64 + 1.0).ln() * 6.0;
    let float_penalty = activity.total_float_hours.min(100.0) * 0.05;
    let score = critical_component * 100.0
        + capability_component
        + priority_component
        + downstream_component
        - float_penalty;

    let mut reasons = Vec::new();
    reasons.push(
        "start gates are satisfied: every FS/SS predecessor event and positive lag has elapsed"
            .into(),
    );
    for release in &claim.provisional {
        reasons.push(format!(
            "provisional: {} attempt #{} is only submitted; a start relies on it, and verification still requires its verified finish",
            release.key, release.attempt
        ));
    }
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
    if let Some(value) = criticality {
        reasons.push(format!(
            "critical in {:.0}% of schedule simulations",
            value * 100.0
        ));
    } else if activity.critical {
        reasons.push("on the deterministic critical path".into());
    }
    if downstream_count > 0 {
        reasons.push(format!("unblocks {downstream_count} downstream work items"));
    }
    if work.priority <= Priority::P1 {
        reasons.push(format!("human priority is {:?}", work.priority));
    }
    if !work.capabilities.is_empty() {
        reasons.push(if capability_match {
            "requested capabilities match".into()
        } else {
            "requested capabilities do not fully match".into()
        });
    }

    NextWorkCandidate {
        work: work.clone(),
        score,
        critical: activity.critical,
        criticality,
        total_float_hours: activity.total_float_hours,
        downstream_count,
        capability_match,
        reasons,
    }
}
