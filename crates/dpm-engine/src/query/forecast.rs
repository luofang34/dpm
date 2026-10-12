//! The `status` risk forecast: milestone percentiles, the probability of finishing by a requested
//! horizon, and the analytic PERT approximation reported beside the sampled forecast.
//!
//! The horizon is a query parameter, never plan state, so the same snapshot answers any horizon.

use crate::EngineError;
use dpm_model::{Key, Plan, Timeline, WorkItemId};
use dpm_schedule::{
    PERT_APPROXIMATION_NOTE, PertApproximation, RemainingOptions, SimulationSummary,
    pert_approximation_remaining_with,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Sampled and analytic schedule risk of the remaining work.
pub struct RiskForecast {
    /// Requested horizon in elapsed hours after the clock reading; null when none was requested.
    pub horizon_hours: Option<f64>,
    /// Fraction of seeded simulations finishing the project by the horizon; the forecast.
    pub probability_by_horizon: Option<f64>,
    /// Spread of the project finish, analytic and sampled.
    pub spread: ForecastSpread,
    /// Classical PERT normal over one mean critical path, for comparison only.
    pub pert_approximation: PertApproximationReport,
    /// Sampled finish of every applicable milestone, in key order.
    pub milestones: Vec<MilestoneForecast>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
/// Two measures of the finish's spread, which differ wherever near-critical paths matter.
pub struct ForecastSpread {
    /// Analytic standard deviation along the mean critical path, from `((P − O) / 6)²` variances.
    pub pert_sigma_hours: f64,
    /// Standard deviation of the sampled project finishes.
    pub sampled_std_dev_hours: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// The analytic approximation and the path it follows.
pub struct PertApproximationReport {
    /// Deterministic remaining finish; without calendars the path's PERT means plus lags.
    pub mean_hours: f64,
    /// Sum of the classical PERT variances along the path.
    pub variance_hours2: f64,
    /// Square root of the variance.
    pub sigma_hours: f64,
    /// `Φ((H − mean) / σ)` for the requested horizon; null without one.
    pub probability_by_horizon: Option<f64>,
    /// The mean critical path, first activity first.
    pub path: Vec<PathStep>,
    /// What the approximation ignores and which way that biases it.
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// One activity on the mean critical path.
pub struct PathStep {
    /// Stable identity.
    pub id: WorkItemId,
    /// Human-readable key.
    pub key: Key,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Sampled finish of one milestone, in elapsed hours after the clock reading.
pub struct MilestoneForecast {
    /// Stable identity.
    pub id: WorkItemId,
    /// Human-readable key.
    pub key: Key,
    /// Median sampled finish.
    pub p50_finish_hours: f64,
    /// 80th percentile sampled finish.
    pub p80_finish_hours: f64,
    /// 95th percentile sampled finish.
    pub p95_finish_hours: f64,
    /// Fraction of seeded simulations reaching the milestone by the horizon; null without one.
    pub probability_by_horizon: Option<f64>,
}

/// A horizon must be a finite, non-negative number of hours after the clock reading.
pub(crate) fn validate_horizon(horizon_hours: Option<f64>) -> Result<(), EngineError> {
    match horizon_hours {
        Some(hours) if !hours.is_finite() || hours < 0.0 => Err(EngineError::InvalidCommand {
            entity: "status horizon".into(),
            reason: format!("horizon must be finite and at least 0 hours, got {hours}"),
        }),
        _ => Ok(()),
    }
}

/// The forecast for a status whose simulation ran; `forecast` is the plan the simulation sampled,
/// and `plan` the authoritative one whose keys are reported.
pub(crate) fn risk_forecast(
    plan: &Plan,
    (forecast, timeline, options): (&Plan, &Timeline, RemainingOptions),
    simulation: &SimulationSummary,
    horizon_hours: Option<f64>,
) -> Result<RiskForecast, EngineError> {
    let approximation = pert_approximation_remaining_with(forecast, timeline, options)?;
    let key = |id: &WorkItemId| plan.work_items.get(id).map(|w| w.key.clone());
    let mut milestones: Vec<MilestoneForecast> = simulation
        .milestones
        .iter()
        .filter_map(|(id, finish)| {
            Some(MilestoneForecast {
                id: *id,
                key: key(id)?,
                p50_finish_hours: finish.p50_finish_hours,
                p80_finish_hours: finish.p80_finish_hours,
                p95_finish_hours: finish.p95_finish_hours,
                probability_by_horizon: horizon_hours.map(|h| finish.probability_by(h)),
            })
        })
        .collect();
    milestones.sort_by(|a, b| a.key.natural_cmp(&b.key));
    Ok(RiskForecast {
        horizon_hours,
        probability_by_horizon: horizon_hours.map(|h| simulation.probability_by(h)),
        spread: ForecastSpread {
            pert_sigma_hours: approximation.sigma_hours,
            sampled_std_dev_hours: simulation.sampled_std_dev_hours,
        },
        pert_approximation: report(&approximation, horizon_hours, key),
        milestones,
    })
}

fn report(
    approximation: &PertApproximation,
    horizon_hours: Option<f64>,
    key: impl Fn(&WorkItemId) -> Option<Key>,
) -> PertApproximationReport {
    PertApproximationReport {
        mean_hours: approximation.mean_hours,
        variance_hours2: approximation.variance_hours2,
        sigma_hours: approximation.sigma_hours,
        probability_by_horizon: horizon_hours.map(|h| approximation.probability_by(h)),
        path: approximation
            .path
            .iter()
            .filter_map(|id| {
                Some(PathStep {
                    id: *id,
                    key: key(id)?,
                })
            })
            .collect(),
        note: PERT_APPROXIMATION_NOTE.into(),
    }
}

#[cfg(test)]
mod tests;
