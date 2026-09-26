use crate::readiness::ready_with_completion;
use crate::{
    EngineError, completion, decisions_resolved, dependencies_satisfied, is_ready, show_work,
};
use dpm_model::{DecisionStatus, Plan, Priority, WorkItem, WorkItemId, WorkStatus};
use dpm_schedule::{Schedule, SimulationConfig, deterministic_remaining, simulate_remaining};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[derive(Debug, Clone, Serialize, Deserialize)]
/// Execution counts and remaining schedule projections.
pub struct StatusSummary {
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
    /// Count of completed tasks and derived aggregate completions.
    pub complete: usize,
    /// Number of unresolved gates.
    pub open_decisions: usize,
    /// Deterministic remaining project duration in elapsed hours.
    pub expected_finish_hours: f64,
    /// Median sampled completion time in elapsed hours.
    pub p50_finish_hours: Option<f64>,
    /// 80th percentile sampled completion time in elapsed hours.
    pub p80_finish_hours: Option<f64>,
}

/// Project validated lifecycle counts and remaining duration.
pub fn status(plan: &Plan, probabilistic: bool) -> Result<StatusSummary, EngineError> {
    plan.validate()?;
    let done = completion(plan);
    let schedule = deterministic_remaining(plan)?;
    let simulation = if probabilistic && plan.work_items.values().any(|w| w.estimate.is_some()) {
        Some(simulate_remaining(
            plan,
            SimulationConfig {
                iterations: 2_000,
                ..SimulationConfig::default()
            },
        )?)
    } else {
        None
    };

    Ok(StatusSummary {
        progress: crate::progress(plan)?.overall,
        revision: plan.revision,
        total_work: plan.work_items.len(),
        ready: plan
            .work_items
            .values()
            .filter(|w| ready_with_completion(plan, w, &done))
            .count(),
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
        complete: done.len(),
        open_decisions: plan
            .decisions
            .values()
            .filter(|decision| decision.status == DecisionStatus::Open)
            .count(),
        expected_finish_hours: schedule.project_finish_hours,
        p50_finish_hours: simulation.as_ref().map(|s| s.p50_finish_hours),
        p80_finish_hours: simulation.as_ref().map(|s| s.p80_finish_hours),
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

/// Recommend only ready tasks compatible with the supplied capabilities.
pub fn next_work(
    plan: &Plan,
    query: &NextWorkQuery,
) -> Result<Vec<NextWorkCandidate>, EngineError> {
    plan.validate()?;
    let done = completion(plan);
    let schedule = deterministic_remaining(plan)?;
    let downstream = downstream_counts(plan);
    let simulation = if query.use_probabilistic_criticality
        && plan.work_items.values().any(|w| w.estimate.is_some())
    {
        Some(simulate_remaining(
            plan,
            SimulationConfig {
                iterations: 2_000,
                ..SimulationConfig::default()
            },
        )?)
    } else {
        None
    };

    let mut candidates = plan
        .work_items
        .values()
        .filter(|work| ready_with_completion(plan, work, &done))
        .filter(|work| {
            query.capabilities.is_empty() || work.capabilities.is_subset(&query.capabilities)
        })
        .map(|work| {
            candidate(
                work,
                query,
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
    reasons.push("all predecessor work is complete".into());
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

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Execution contract, prerequisites, and derived reason for a work item.
pub struct WorkExplanation {
    /// Execution percentage and independent completion condition.
    pub progress: crate::ProgressSummary,
    /// Resolved requirements, gates, evidence and related work for the execution contract.
    pub context: crate::ExecutionContext,
    /// Work-item projection represented by this result.
    pub work: WorkItem,
    /// Whether this task can be claimed now.
    pub ready: bool,
    /// Direct predecessor work items.
    pub predecessors: Vec<WorkItem>,
    /// Count of distinct transitively dependent work items.
    pub downstream_count: usize,
    /// Derived timing of this activity when available.
    pub schedule: Option<dpm_schedule::ActivitySchedule>,
    /// Fraction of sampled schedules in which each activity is critical.
    pub criticality: Option<f64>,
    /// Readiness, blocker, gate, lifecycle, and schedule explanations.
    pub why_now: Vec<String>,
}

/// Explain a validated work item, including derived aggregate completion.
pub fn explain_work(plan: &Plan, work: WorkItemId) -> Result<WorkExplanation, EngineError> {
    let item = show_work(plan, work)?;
    let schedule: Schedule = deterministic_remaining(plan)?;
    let simulation = simulate_remaining(
        plan,
        SimulationConfig {
            iterations: 2_000,
            ..SimulationConfig::default()
        },
    )?;
    let predecessors = plan
        .dependencies
        .iter()
        .filter(|dep| dep.successor == work)
        .filter_map(|dep| plan.work_items.get(&dep.predecessor).cloned())
        .collect::<Vec<_>>();
    let downstream_count = downstream_counts(plan).get(&work).copied().unwrap_or(0);
    let ready = is_ready(plan, &item);
    let activity = schedule.activities.get(&work).cloned();
    let criticality = simulation.criticality.get(&work).copied();

    let mut why_now = Vec::new();
    if ready {
        why_now.push("work is executable now: all predecessor dependencies are complete".into());
    } else if completion(plan).contains(&work) {
        why_now.push("work is complete".into());
    } else if !item.is_executable() {
        why_now.push("aggregate work completes through its prerequisites or children".into());
    } else if item.status == WorkStatus::Blocked {
        why_now.push(format!(
            "work is blocked: {}",
            item.block_reason.as_deref().unwrap_or("no reason recorded")
        ));
    } else if !dependencies_satisfied(plan, work) {
        why_now.push("one or more predecessor dependencies are incomplete".into());
    } else if !decisions_resolved(plan, work) {
        let keys = crate::readiness::blocking_decision_keys(plan, work).join(", ");
        why_now.push(format!("work awaits decision: {keys}"));
    } else {
        why_now.push(format!("work lifecycle state is {:?}", item.status));
    }
    if downstream_count > 0 {
        why_now.push(format!(
            "completion affects {downstream_count} downstream work items"
        ));
    }
    if let Some(value) = criticality {
        why_now.push(format!("schedule criticality is {:.0}%", value * 100.0));
    }

    Ok(WorkExplanation {
        progress: crate::progress(plan)?.work[&work],
        context: crate::context::execution_context(plan, &item),
        work: item,
        ready,
        predecessors,
        downstream_count,
        schedule: activity,
        criticality,
        why_now,
    })
}
