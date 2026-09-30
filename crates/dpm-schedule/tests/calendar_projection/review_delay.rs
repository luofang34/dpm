//! An opt-in review delay after the work of every task still awaiting verification.

use super::{Builder, berlin, close, hours, zone};
use dpm_model::{ActorKind, DependencyKind, LagBasis, Timeline};
use dpm_schedule::{
    RemainingOptions, ScheduleError, SimulationConfig, deterministic_remaining,
    deterministic_remaining_with, simulate_remaining_with,
};

fn delayed(hours: f64) -> RemainingOptions {
    RemainingOptions {
        review_delay_hours: hours,
    }
}

/// Agent work ends at 11:00 on a Monday; a human review then waits the delay and, when that ends
/// outside working time, the next working moment of the verifier's calendar.
#[test]
fn a_review_delay_ends_on_the_verifiers_calendar() {
    let mut plan = Builder::new(zone());
    plan.task(1, 2.0, ActorKind::Agent);
    let now = berlin(2, 9);
    let timeline = Timeline::at(&plan.plan, now);
    let finish = |delay| {
        deterministic_remaining_with(&plan.plan, &timeline, delayed(delay))
            .expect("schedule")
            .project_finish_hours
    };
    close(finish(0.0), 2.0);
    close(finish(5.0), hours(now, berlin(2, 16)));
    close(finish(8.0), hours(now, berlin(3, 8)));
}

/// Without calendars the delay is elapsed time after each unverified task and successors wait for
/// the delayed verification; a zero delay is the plain projection, and a plan without calendars
/// reports none.
#[test]
fn a_review_delay_without_calendars_adds_elapsed_hours_per_review() {
    let mut plan = Builder::new(None);
    let first = plan.task(1, 4.0, ActorKind::Human);
    let second = plan.task(2, 3.0, ActorKind::Human);
    plan.link(
        first,
        second,
        DependencyKind::FinishStart,
        0.0,
        LagBasis::Elapsed,
    );
    let now = berlin(2, 9);
    let timeline = Timeline::at(&plan.plan, now);
    let plain = deterministic_remaining(&plan.plan, now).expect("plain");
    let zero = deterministic_remaining_with(&plan.plan, &timeline, delayed(0.0)).expect("zero");
    assert_eq!(
        serde_json::to_value(&plain).expect("json"),
        serde_json::to_value(&zero).expect("json")
    );

    let schedule =
        deterministic_remaining_with(&plan.plan, &timeline, delayed(2.0)).expect("delayed");
    close(schedule.project_finish_hours, 11.0);
    let later = schedule.activities.get(&second).expect("second");
    close(later.earliest_start_hours, 6.0);
    assert!(later.calendar.is_none());
    let config = SimulationConfig {
        iterations: 50,
        ..SimulationConfig::default()
    };
    let sampled =
        simulate_remaining_with(&plan.plan, config, &timeline, delayed(2.0)).expect("sampled");
    close(sampled.p95_finish_hours, 11.0);

    for invalid in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            deterministic_remaining_with(&plan.plan, &timeline, delayed(invalid)),
            Err(ScheduleError::InvalidReviewDelay(_))
        ));
    }
}
