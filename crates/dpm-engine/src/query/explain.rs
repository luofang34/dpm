use super::{downstream_counts, simulation_config};
use crate::{EngineError, GateReport, Transition, gates, readiness::show_with};
use chrono::{DateTime, Utc};
use dpm_model::{Applicability, Plan, Timeline, WorkItem, WorkItemId};
use dpm_schedule::{deterministic_remaining, simulate_remaining};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Execution contract, prerequisites, and derived reason for a work item.
pub struct WorkExplanation {
    /// Claim eligibility, the same report `next` and `status` use.
    pub gates: GateReport,
    /// Eligibility of every lifecycle transition, so FF/SF finish gates are visible before a claim.
    #[serde(default)]
    pub transitions: BTreeMap<Transition, GateReport>,
    /// Execution percentage, independent completion condition and completion time.
    pub progress: crate::ProgressSummary,
    /// Resolved requirements, gates, evidence and related work for the execution contract.
    pub context: crate::ExecutionContext,
    /// Work-item projection represented by this result, including recorded event times.
    pub work: WorkItem,
    /// Whether this task can be claimed now.
    pub ready: bool,
    /// Whether the work belongs to the active graph, derived from conditions, joins and choices.
    #[serde(default = "applicable")]
    pub applicability: Applicability,
    /// Direct predecessor work items.
    pub predecessors: Vec<WorkItem>,
    /// Count of distinct transitively dependent work items.
    pub downstream_count: usize,
    /// Derived timing of this activity when available.
    pub schedule: Option<dpm_schedule::ActivitySchedule>,
    /// Fraction of sampled schedules in which each activity is critical.
    pub criticality: Option<f64>,
    /// Readiness, blocker, gate, lifecycle, lag and schedule explanations.
    pub why_now: Vec<String>,
}

/// Explain a validated work item at an adapter-supplied time, including derived completion.
pub fn explain_work(
    plan: &Plan,
    work: WorkItemId,
    now: DateTime<Utc>,
) -> Result<WorkExplanation, EngineError> {
    plan.validate()?;
    let timeline = Timeline::at(plan, now);
    let item = show_with(plan, work, &timeline)?;
    let schedule = deterministic_remaining(plan, now)?;
    let simulation = simulate_remaining(plan, simulation_config(), now)?;
    let predecessors = plan
        .dependencies
        .iter()
        .filter(|dep| dep.successor == work)
        .filter_map(|dep| plan.work_items.get(&dep.predecessor).cloned())
        .collect::<Vec<_>>();
    let downstream_count = downstream_counts(plan, &timeline)
        .get(&work)
        .copied()
        .unwrap_or(0);
    let authoritative = plan
        .work_items
        .get(&work)
        .ok_or(EngineError::MissingWorkItem(work))?;
    let transitions: BTreeMap<_, _> = Transition::ALL
        .into_iter()
        .map(|t| (t, gates::evaluate(plan, authoritative, t, &timeline)))
        .collect();
    let gates = transitions
        .get(&Transition::Claim)
        .cloned()
        .unwrap_or_else(|| gates::evaluate(plan, authoritative, Transition::Claim, &timeline));
    let ready = gates.ready;
    let criticality = simulation.criticality.get(&work).copied();
    let why_now = why_now(
        plan,
        authoritative,
        &timeline,
        &transitions,
        downstream_count,
        criticality,
    );

    Ok(WorkExplanation {
        gates,
        transitions,
        progress: crate::progress::progress_with(plan, &timeline).work[&work],
        context: crate::context::execution_context(plan, &item),
        work: item,
        ready,
        applicability: timeline.applicability(work).clone(),
        predecessors,
        downstream_count,
        schedule: schedule.activities.get(&work).cloned(),
        criticality,
        why_now,
    })
}

fn applicable() -> Applicability {
    Applicability::Applicable
}

fn why_now(
    plan: &Plan,
    work: &WorkItem,
    timeline: &Timeline,
    transitions: &BTreeMap<Transition, GateReport>,
    downstream_count: usize,
    criticality: Option<f64>,
) -> Vec<String> {
    let mut why_now = Vec::new();
    let current = Transition::ALL
        .into_iter()
        .find(|t| t.lifecycle() == work.status)
        .and_then(|t| transitions.get(&t).map(|report| (t, report)));
    if timeline.completed_at(work.id).is_some() {
        why_now.push("work is complete".into());
    } else if let Some((transition, report)) = current {
        if report.ready {
            why_now.push(match transition {
                Transition::Claim => "work can be claimed now: every FS/SS predecessor event and positive lag has elapsed and no decision gates it".into(),
                other => format!("work can {other} now: every gate for this transition is satisfied"),
            });
        } else {
            why_now.extend(report.reasons());
        }
    } else {
        why_now.extend(
            transitions
                .get(&Transition::Claim)
                .map(GateReport::reasons)
                .unwrap_or_default(),
        );
    }
    for dep in plan
        .enforced_dependencies()
        .filter(|d| d.successor == work.id && d.lag_hours < 0.0)
    {
        let key = plan
            .work_items
            .get(&dep.predecessor)
            .map_or_else(String::new, |w| w.key.to_string());
        why_now.push(format!(
            "{} {}h lead from {key} shapes the schedule only; execution still waits for the predecessor event",
            dep.kind.abbreviation(),
            dep.lag_hours
        ));
    }
    if downstream_count > 0 {
        why_now.push(format!(
            "completion affects {downstream_count} downstream work items"
        ));
    }
    if let Some(value) = criticality {
        why_now.push(format!("schedule criticality is {:.0}%", value * 100.0));
    }
    why_now
}
