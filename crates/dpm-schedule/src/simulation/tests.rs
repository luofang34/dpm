use super::*;
use dpm_model::{ActorId, ThreePointEstimate, WorkStatus};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

#[test]
fn seeded_simulation_is_reproducible_ordered_and_bounded() {
    let plan = fixture();
    let config = SimulationConfig {
        iterations: 500,
        seed: 1234,
    };
    let a = simulate(&plan, config).expect("simulate");
    let b = simulate(&plan, config).expect("repeat");
    assert_eq!(a.p50_finish_hours, b.p50_finish_hours);
    assert_eq!(a.p80_finish_hours, b.p80_finish_hours);
    assert_eq!(a.p95_finish_hours, b.p95_finish_hours);
    assert_eq!(a.criticality, b.criticality);
    assert!(a.p50_finish_hours <= a.p80_finish_hours && a.p80_finish_hours <= a.p95_finish_hours);
    assert!(a.criticality.values().all(|v| (0.0..=1.0).contains(v)));
    assert_eq!(a.criticality.len(), plan.work_items.len());
    assert!(
        simulate(
            &plan,
            SimulationConfig {
                iterations: 0,
                seed: 0
            }
        )
        .is_err()
    );
}

#[test]
fn completed_plan_has_zero_remaining_duration_even_with_lag() {
    let mut plan = fixture();
    for work in plan.work_items.values_mut().filter(|w| w.is_executable()) {
        work.status = WorkStatus::Verified;
        work.owner = Some(ActorId::agent("owner"));
    }
    for dependency in &mut plan.dependencies {
        dependency.lag_hours = 5.0;
    }
    let result = simulate_remaining(
        &plan,
        SimulationConfig {
            iterations: 10,
            seed: 0,
        },
    )
    .expect("remaining");
    assert_eq!(result.p95_finish_hours, 0.0);
}

#[test]
fn extreme_finite_triangular_samples_stay_inside_estimate_bounds() {
    let mut rng = XorShift64::new(0);
    let estimate = ThreePointEstimate {
        optimistic_hours: 0.0,
        likely_hours: f64::MAX / 2.0,
        pessimistic_hours: f64::MAX,
    };
    for _ in 0..1000 {
        let sample = sample_triangular(
            &mut rng,
            estimate.optimistic_hours,
            estimate.likely_hours,
            estimate.pessimistic_hours,
        );
        assert!(sample.is_finite());
        assert!((0.0..=f64::MAX).contains(&sample));
    }
    assert_eq!(sample_triangular(&mut rng, 4.0, 4.0, 4.0), 4.0);
}
