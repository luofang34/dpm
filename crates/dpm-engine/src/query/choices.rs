//! Projections of conditional work: what is outside the active graph, and one forecast per
//! combination of open choices instead of a single percentile over mutually exclusive branches.

use super::simulation_config;
use crate::EngineError;
use chrono::{DateTime, Utc};
use dpm_model::{Applicability, DecisionId, DecisionStatus, Key, Plan, Timeline};
use dpm_schedule::{deterministic_remaining, simulate_remaining};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Scenario forecasts are computed only up to this many option combinations.
pub const MAX_SCENARIOS: usize = 16;

/// Work outside the active graph, with the derived reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InapplicableWork {
    /// Human-readable work key.
    pub key: Key,
    /// Why the work cannot take transitions or enter the forecast.
    pub applicability: Applicability,
}

/// Open decisions whose options decide which conditional work applies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenChoices {
    /// Open decisions, by key, that condition work.
    pub decisions: Vec<Key>,
    /// Number of option combinations across those decisions.
    pub scenario_count: usize,
    /// One forecast per combination; empty when `scenario_count` exceeds [`MAX_SCENARIOS`].
    pub scenarios: Vec<ScenarioForecast>,
}

/// Remaining forecast if the open decisions select these options now.
///
/// No probability is assigned to a scenario, so scenarios are never merged into one percentile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioForecast {
    /// Option key selected for each open decision.
    pub choices: BTreeMap<Key, String>,
    /// Deterministic remaining duration of this scenario's active graph, in elapsed hours.
    pub expected_finish_hours: f64,
    /// Median sampled completion of this scenario, when requested.
    pub p50_finish_hours: Option<f64>,
    /// 80th percentile sampled completion of this scenario, when requested.
    pub p80_finish_hours: Option<f64>,
    /// 95th percentile sampled completion of this scenario, when requested.
    pub p95_finish_hours: Option<f64>,
    /// Work that could not proceed in this scenario without a plan change.
    pub stranded: Vec<Key>,
    /// Tasks this scenario's forecast counts as 0 h because they have no estimate, in key order.
    #[serde(default)]
    pub unestimated: Vec<Key>,
}

pub(crate) fn not_applicable(plan: &Plan, timeline: &Timeline) -> Vec<InapplicableWork> {
    let mut work: Vec<_> = plan
        .work_items
        .values()
        .filter(|w| !timeline.applicability(w.id).is_applicable())
        .map(|w| InapplicableWork {
            key: w.key.clone(),
            applicability: timeline.applicability(w.id).clone(),
        })
        .collect();
    work.sort_by(|a, b| a.key.natural_cmp(&b.key));
    work
}

/// Open decisions that condition work, in key order.
fn open_decisions(plan: &Plan) -> Vec<(DecisionId, Key, Vec<String>)> {
    let ids: BTreeSet<_> = plan
        .work_items
        .values()
        .filter_map(|w| w.condition.as_ref())
        .filter_map(|c| plan.effective_decision(c.decision))
        .filter(|d| d.status == DecisionStatus::Open)
        .map(|d| d.id)
        .collect();
    let mut open: Vec<_> = ids
        .into_iter()
        .filter_map(|id| plan.decisions.get(&id))
        .map(|d| {
            let options = d.options.iter().map(|o| o.key.clone()).collect();
            (d.id, d.key.clone(), options)
        })
        .collect();
    open.sort_by(|a, b| a.1.cmp(&b.1));
    open
}

/// Forecast each combination of open choices, or `None` when no open choice conditions work.
pub(crate) fn open_choices(
    plan: &Plan,
    probabilistic: bool,
    now: DateTime<Utc>,
) -> Result<Option<OpenChoices>, EngineError> {
    let open = open_decisions(plan);
    if open.is_empty() {
        return Ok(None);
    }
    let scenario_count = open.iter().fold(1_usize, |n, (_, _, options)| {
        n.saturating_mul(options.len())
    });
    let mut scenarios = Vec::new();
    if scenario_count <= MAX_SCENARIOS {
        for index in 0..scenario_count {
            let mut rest = index;
            let choices: Vec<_> = open
                .iter()
                .map(|(id, key, options)| {
                    let count = options.len().max(1);
                    let option = options.get(rest % count).cloned().unwrap_or_default();
                    rest /= count;
                    (*id, key.clone(), option)
                })
                .collect();
            scenarios.push(forecast(plan, &choices, probabilistic, now)?);
        }
    }
    Ok(Some(OpenChoices {
        decisions: open.into_iter().map(|(_, key, _)| key).collect(),
        scenario_count,
        scenarios,
    }))
}

fn forecast(
    plan: &Plan,
    choices: &[(DecisionId, Key, String)],
    probabilistic: bool,
    now: DateTime<Utc>,
) -> Result<ScenarioForecast, EngineError> {
    let mut hypothetical = plan.clone();
    for (id, _, option) in choices {
        if let Some(decision) = hypothetical.decisions.get_mut(id) {
            decision.status = DecisionStatus::Decided;
            decision.outcome = Some(option.clone());
            decision.resolved_at = Some(now);
        }
    }
    let schedule = deterministic_remaining(&hypothetical, now)?;
    let simulation = if probabilistic
        && hypothetical
            .work_items
            .values()
            .any(|w| w.estimate.is_some())
    {
        Some(simulate_remaining(&hypothetical, simulation_config(), now)?)
    } else {
        None
    };
    let mut stranded: Vec<_> = hypothetical
        .applicability()
        .into_iter()
        .filter(|(_, a)| {
            matches!(
                a,
                Applicability::Stranded { .. }
                    | Applicability::ChildrenStranded { .. }
                    | Applicability::EmptyJoin
            )
        })
        .filter_map(|(id, _)| hypothetical.work_items.get(&id).map(|w| w.key.clone()))
        .collect();
    stranded.sort_by(dpm_model::Key::natural_cmp);
    Ok(ScenarioForecast {
        choices: choices
            .iter()
            .map(|(_, key, option)| (key.clone(), option.clone()))
            .collect(),
        expected_finish_hours: schedule.project_finish_hours,
        p50_finish_hours: simulation.as_ref().map(|s| s.p50_finish_hours),
        p80_finish_hours: simulation.as_ref().map(|s| s.p80_finish_hours),
        p95_finish_hours: simulation.as_ref().map(|s| s.p95_finish_hours),
        stranded,
        unestimated: super::unestimated(&hypothetical, &Timeline::at(&hypothetical, now)),
    })
}
