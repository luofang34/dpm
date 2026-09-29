use super::*;
use dpm_model::{Plan, ThreePointEstimate, WorkKind};

fn task(
    optimistic_hours: f64,
    likely_hours: f64,
    pessimistic_hours: f64,
) -> (WorkItemId, WorkItem) {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let mut work = plan
        .work_items
        .values()
        .find(|w| w.is_executable())
        .cloned()
        .expect("task");
    work.schedule.estimate = Some(ThreePointEstimate {
        optimistic_hours,
        likely_hours,
        pessimistic_hours,
    });
    (work.id, work)
}

fn duration(estimate: (f64, f64, f64), elapsed: f64) -> RemainingDuration {
    let (id, work) = task(estimate.0, estimate.1, estimate.2);
    RemainingDuration::of(id, &work, false, elapsed).expect("valid")
}

fn sample_mean(duration: &RemainingDuration, count: usize) -> (f64, f64, f64) {
    let mut rng = XorShift64::new(0x40);
    let samples: Vec<f64> = (0..count).map(|_| duration.sample(&mut rng)).collect();
    let low = samples.iter().copied().fold(f64::INFINITY, f64::min);
    let high = samples.iter().copied().fold(0.0, f64::max);
    (samples.iter().sum::<f64>() / count as f64, low, high)
}

/// `E[D - e | D > e]` for beta-PERT 1/2/9 h (shapes 1.5 and 4.5) by fine Simpson integration,
/// independent of the tabulated cells.
fn conditional_remaining_1_2_9(elapsed: f64) -> f64 {
    let density = |x: f64| libm::pow(x, 0.5) * libm::pow(1.0 - x, 3.5);
    let from = (elapsed - 1.0) / 8.0;
    let steps = 20_000;
    let h = (1.0 - from) / steps as f64;
    let (mut mass, mut moment) = (0.0, 0.0);
    for i in 0..=steps {
        let x = (from + h * i as f64).min(1.0);
        let weight = if i == 0 || i == steps {
            1.0
        } else if i % 2 == 1 {
            4.0
        } else {
            2.0
        };
        mass += weight * density(x);
        moment += weight * density(x) * x;
    }
    1.0 + 8.0 * moment / mass - elapsed
}

#[test]
fn unstarted_work_contributes_its_whole_pert_expectation() {
    // 1/2/9 h: (1 + 4 * 2 + 9) / 6 = 3 h, which the samples reproduce as their mean.
    let whole = duration((1.0, 2.0, 9.0), 0.0);
    assert_eq!(whole.expected(), 3.0);
    let (mean, low, high) = sample_mean(&whole, 100_000);
    assert!((mean - 3.0).abs() < 0.02, "{mean}");
    assert!(low >= 1.0 && high <= 9.0);
}

#[test]
fn elapsed_time_below_the_optimistic_bound_is_simply_subtracted() {
    // Half an hour into a task that takes at least 1 h rules nothing out: 3 - 0.5 = 2.5 h remain.
    let started = duration((1.0, 2.0, 9.0), 0.5);
    assert_eq!(started.expected(), 2.5);
    let (mean, low, high) = sample_mean(&started, 100_000);
    assert!((mean - 2.5).abs() < 0.02, "{mean}");
    assert!(low >= 0.5 && high <= 8.5);
}

#[test]
fn started_work_keeps_the_conditional_mean_of_what_remains() {
    // Five hours into 1/2/9 h the PERT expectation (3 h) has passed, so `expected - elapsed` would
    // project the task as finished. Conditional on still running, about 0.76 h remain.
    let started = duration((1.0, 2.0, 9.0), 5.0);
    let expected = started.expected();
    let exact = conditional_remaining_1_2_9(5.0);
    assert!((expected - exact).abs() < 1e-3, "{expected} vs {exact}");
    assert!(expected > 0.7 && expected < 0.8, "{expected}");
    let (mean, low, high) = sample_mean(&started, 100_000);
    assert!((mean - expected).abs() < 0.01, "{mean} vs {expected}");
    assert!(low > 0.0 && high <= 4.0, "{low}..{high}");
}

#[test]
fn conditional_remaining_is_never_below_the_naive_subtraction() {
    let mut previous_finish = 0.0;
    for tenth in 0..95 {
        let elapsed = f64::from(tenth) / 10.0;
        let remaining = duration((1.0, 2.0, 9.0), elapsed).expected();
        assert!(remaining >= (3.0 - elapsed).max(0.0) - 1e-9, "{elapsed}");
        assert!(remaining > 0.0 || elapsed >= 9.0, "{elapsed}");
        let finish = elapsed + remaining;
        assert!(finish >= previous_finish - 1e-9, "{elapsed}: {finish}");
        previous_finish = finish;
        if elapsed > 1.0 && elapsed < 9.0 {
            let exact = conditional_remaining_1_2_9(elapsed);
            assert!(
                (remaining - exact).abs() < 2e-3,
                "{elapsed}: {remaining} vs {exact}"
            );
        }
    }
}

#[test]
fn work_past_its_pessimistic_bound_projects_to_finish_at_the_origin() {
    for elapsed in [9.0, 30.0] {
        let overrun = duration((1.0, 2.0, 9.0), elapsed);
        assert_eq!(overrun.expected(), 0.0);
        assert_eq!(sample_mean(&overrun, 10).2, 0.0);
    }
}

#[test]
fn exact_estimates_stay_exact() {
    for (elapsed, remaining) in [(0.0, 4.0), (1.5, 2.5), (6.0, 0.0)] {
        let exact = duration((4.0, 4.0, 4.0), elapsed);
        assert_eq!(exact.expected(), remaining);
        let (mean, low, high) = sample_mean(&exact, 10);
        assert_eq!((mean, low, high), (remaining, remaining, remaining));
    }
}

#[test]
fn done_unestimated_and_aggregate_work_contribute_nothing() {
    let (id, mut work) = task(1.0, 2.0, 9.0);
    let done = RemainingDuration::of(id, &work, true, 0.0).expect("done");
    assert_eq!(done.expected(), 0.0);
    work.kind = WorkKind::Milestone;
    assert_eq!(
        RemainingDuration::of(id, &work, false, 0.0)
            .expect("milestone")
            .expected(),
        0.0
    );
    work.kind = WorkKind::Task;
    work.schedule.estimate = None;
    assert_eq!(
        RemainingDuration::of(id, &work, false, 3.0)
            .expect("unestimated")
            .expected(),
        0.0
    );
}

#[test]
fn invalid_inputs_are_rejected() {
    let (id, work) = task(1.0, 2.0, 9.0);
    assert!(RemainingDuration::of(id, &work, false, f64::NAN).is_err());
    let (id, work) = task(3.0, 2.0, 9.0);
    assert!(RemainingDuration::of(id, &work, false, 0.0).is_err());
}
