use super::*;
use crate::{explain_work, status, status_at_horizon};
use chrono::{DateTime, TimeZone, Utc};

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
}

fn fixture(name: &str) -> Plan {
    let text = match name {
        "conditional" => include_str!("../../../../../tests/support/conditional-plan.json"),
        _ => include_str!("../../../../../tests/support/execution-plan.json"),
    };
    serde_json::from_str(text).expect("fixture")
}

/// TEST-A (2/4/6) -> TEST-B (10/20/30) -> TEST-C (20/40/60) -> TEST-E (4/8/12) -> TEST-F (2/4/6)
/// -> TEST-M1 is the 76 h mean critical path; TEST-D's branch through it is 48 h. Its classical
/// variance is (2² + 10² + 20² + 4² + 2²) / 9 = 524 / 9.
const PATH_VARIANCE: f64 = 524.0 / 9.0;

#[test]
fn forecast_reports_the_mean_critical_path_and_withholds_probabilities_without_a_horizon() {
    let plan = fixture("execution");
    let summary = status(&plan, true, at()).expect("status");
    let forecast = summary.forecast.expect("forecast beside the percentiles");
    assert_eq!(forecast.horizon_hours, None);
    assert_eq!(forecast.probability_by_horizon, None);
    let pert = &forecast.pert_approximation;
    assert_eq!(pert.probability_by_horizon, None);
    let keys: Vec<&str> = pert.path.iter().map(|step| step.key.0.as_str()).collect();
    assert_eq!(
        keys,
        ["TEST-A", "TEST-B", "TEST-C", "TEST-E", "TEST-F", "TEST-M1"]
    );
    assert!((pert.mean_hours - summary.expected_finish_hours).abs() < 1e-9);
    assert!((pert.mean_hours - 76.0).abs() < 1e-9);
    assert!((pert.variance_hours2 - PATH_VARIANCE).abs() < 1e-9);
    assert!((forecast.spread.pert_sigma_hours - PATH_VARIANCE.sqrt()).abs() < 1e-9);
    assert!(forecast.spread.sampled_std_dev_hours > 0.0);
    assert!(pert.note.contains("approximation") && pert.note.contains("overstates"));
    let [milestone] = forecast.milestones.as_slice() else {
        panic!("one milestone: {:?}", forecast.milestones);
    };
    assert_eq!(milestone.key.0, "TEST-M1");
    assert_eq!(milestone.probability_by_horizon, None);
    assert!(milestone.p50_finish_hours <= milestone.p95_finish_hours);
}

#[test]
fn a_horizon_adds_sampled_and_approximate_probabilities() {
    let plan = fixture("execution");
    let summary = status_at_horizon(&plan, true, at(), Some(80.0)).expect("status");
    let forecast = summary.forecast.expect("forecast");
    assert_eq!(forecast.horizon_hours, Some(80.0));
    let sampled = forecast.probability_by_horizon.expect("sampled");
    assert!(sampled > 0.0 && sampled < 1.0);
    let z = (80.0 - 76.0) / PATH_VARIANCE.sqrt();
    let expected = 0.5 * (1.0 + libm::erf(z / std::f64::consts::SQRT_2));
    let approximate = forecast
        .pert_approximation
        .probability_by_horizon
        .expect("approximate");
    assert!((approximate - expected).abs() < 1e-12);
    // The milestone closes the plan, so it is reached exactly when the project finishes.
    assert_eq!(forecast.milestones[0].probability_by_horizon, Some(sampled));
    // The horizon is a query parameter: it changes no other part of the status.
    let plain = status(&plan, true, at()).expect("status");
    assert_eq!(summary.p80_finish_hours, plain.p80_finish_hours);
    assert_eq!(summary.expected_finish_hours, plain.expected_finish_hours);
}

#[test]
fn forecast_is_withheld_with_the_percentiles() {
    let plan = fixture("execution");
    let deterministic = status_at_horizon(&plan, false, at(), Some(80.0)).expect("status");
    assert!(deterministic.p50_finish_hours.is_none() && deterministic.forecast.is_none());
    let conditional = fixture("conditional");
    let open = status_at_horizon(&conditional, true, at(), Some(80.0)).expect("status");
    assert!(open.open_choices.is_some());
    assert!(open.p50_finish_hours.is_none() && open.forecast.is_none());
}

#[test]
fn a_horizon_must_be_finite_and_not_negative() {
    let plan = fixture("execution");
    for horizon in [-1.0, f64::NAN, f64::INFINITY] {
        let refused = status_at_horizon(&plan, true, at(), Some(horizon));
        assert!(
            matches!(refused, Err(EngineError::InvalidCommand { .. })),
            "{horizon}: {refused:?}"
        );
    }
    assert!(status_at_horizon(&plan, true, at(), Some(0.0)).is_ok());
}

#[test]
fn explain_reports_drag_and_classical_variance() {
    let plan = fixture("execution");
    let id = |key: &str| plan.find_work_by_key(key).expect("work").id;
    // Without TEST-A's 4 h the 76 h path finishes at 72 h; TEST-D (16/32/48) floats 28 h.
    let first = explain_work(&plan, id("TEST-A"), at()).expect("explain");
    assert!((first.drag_hours.expect("drag") - 4.0).abs() < 1e-9);
    assert!((first.pert_variance_hours2.expect("variance") - 4.0 / 9.0).abs() < 1e-12);
    let parallel = explain_work(&plan, id("TEST-D"), at()).expect("explain");
    assert_eq!(parallel.drag_hours, Some(0.0));
    assert!((parallel.pert_variance_hours2.expect("variance") - 1024.0 / 36.0).abs() < 1e-12);
}
