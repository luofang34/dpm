use super::*;
use dpm_model::{ActorId, WorkStatus};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn approximation(mean: f64, sigma: f64) -> PertApproximation {
    PertApproximation {
        mean_hours: mean,
        variance_hours2: sigma * sigma,
        sigma_hours: sigma,
        path: Vec::new(),
        activity_variance_hours2: BTreeMap::new(),
    }
}

#[test]
fn probability_is_the_normal_distribution_function_and_a_step_without_variance() {
    let normal = approximation(44.0, 3.0);
    assert!((normal.probability_by(44.0) - 0.5).abs() < 1e-12);
    assert!((normal.probability_by(47.0) - 0.841_344_746).abs() < 1e-8);
    assert!((normal.probability_by(41.0) - 0.158_655_254).abs() < 1e-8);
    let exact = approximation(10.0, 0.0);
    assert_eq!(exact.probability_by(9.5), 0.0);
    assert_eq!(exact.probability_by(10.0), 1.0);
}

#[test]
fn classical_variance_is_a_sixth_of_the_range_squared() {
    let estimate = ThreePointEstimate {
        optimistic_hours: 6.0,
        likely_hours: 9.0,
        pessimistic_hours: 18.0,
    };
    assert_eq!(pert_variance_hours2(estimate), 4.0);
}

#[test]
fn ties_go_to_the_larger_variance_then_the_smaller_id() {
    let (low, high) = (Some(WorkItemId::new()), Some(WorkItemId::new()));
    let (low, high) = if low < high { (low, high) } else { (high, low) };
    assert!(prefers((2.0, high), (1.0, low)));
    assert!(prefers((1.0, low), (1.0, high)));
    assert!(!prefers((1.0, high), (1.0, low)));
}

/// Started work keeps only the range its elapsed time allows, and verified work has no variance,
/// so the remaining approximation's variance shrinks as the plan progresses.
#[test]
fn remaining_variance_narrows_for_started_work_and_vanishes_for_finished_work() {
    let mut plan = fixture();
    let at = |hours| chrono::DateTime::UNIX_EPOCH + chrono::TimeDelta::hours(hours);
    let id_of = |plan: &Plan, key: &str| {
        plan.work_items
            .values()
            .find(|w| w.key == dpm_model::Key::new(key))
            .expect("task")
            .id
    };
    let (a, b) = (id_of(&plan, "TEST-A"), id_of(&plan, "TEST-B"));
    let before = pert_approximation_remaining_at(&plan, &dpm_model::Timeline::at(&plan, at(0)))
        .expect("approximation");
    // TEST-A is 2/4/6 h and TEST-B 10/20/30 h.
    assert!((before.activity_variance_hours2[&a] - 4.0 / 9.0).abs() < 1e-12);
    assert!((before.activity_variance_hours2[&b] - 100.0 / 9.0).abs() < 1e-12);
    if let Some(work) = plan.work_items.get_mut(&a) {
        work.execution.status = WorkStatus::Verified;
        work.execution.owner = Some(ActorId::agent("owner"));
        work.execution.events.started_at = Some(at(0));
        work.execution.events.verified_at = Some(at(4));
    }
    if let Some(work) = plan.work_items.get_mut(&b) {
        work.execution.status = WorkStatus::InProgress;
        work.execution.owner = Some(ActorId::agent("owner"));
        work.execution.events.started_at = Some(at(4));
    }
    // Sixteen hours into TEST-B only 16..30 h remains possible: ((30 - 16) / 6)².
    let after = pert_approximation_remaining_at(&plan, &dpm_model::Timeline::at(&plan, at(20)))
        .expect("approximation");
    assert_eq!(after.activity_variance_hours2[&a], 0.0);
    assert!((after.activity_variance_hours2[&b] - 196.0 / 36.0).abs() < 1e-12);
    assert!(after.variance_hours2 < before.variance_hours2);
    assert!(after.sigma_hours * after.sigma_hours - after.variance_hours2 < 1e-9);
}
