//! Milestone percentiles and horizon probabilities come from the same samples as the project
//! finish, so a seed pins them exactly.

use super::*;

fn milestone(plan: &Plan) -> WorkItemId {
    plan.work_items
        .values()
        .find(|w| w.kind == dpm_model::WorkKind::Milestone)
        .expect("fixture milestone")
        .id
}

#[test]
fn seeded_milestone_percentiles_and_horizon_probabilities_are_pinned() {
    let plan = fixture();
    let config = SimulationConfig {
        iterations: 2_000,
        seed: 1234,
    };
    let summary = simulate(&plan, config).expect("simulate");
    let reached = &summary.milestones[&milestone(&plan)];
    let pinned = |actual: f64, expected: f64| (actual - expected).abs() < 1e-9;
    assert!(pinned(reached.p50_finish_hours, 76.136_661_421_473_32));
    assert!(pinned(reached.p80_finish_hours, 83.486_657_822_199_1));
    assert!(pinned(reached.p95_finish_hours, 89.742_461_931_565_56));
    assert!(pinned(summary.sampled_std_dev_hours, 8.453_236_468_733_77));
    // The interpolated 80th percentile of 2000 samples lies between the 1600th and 1601st.
    assert_eq!(reached.probability_by(reached.p80_finish_hours), 0.8);
    assert!(reached.p50_finish_hours <= reached.p80_finish_hours);
    assert!(reached.p80_finish_hours <= reached.p95_finish_hours);
    assert!(reached.p95_finish_hours <= summary.p95_finish_hours);
    assert_eq!(reached.probability_by(-1.0), 0.0);
    assert_eq!(reached.probability_by(f64::MAX), 1.0);
    assert_eq!(summary.milestones.len(), 1);
}
