//! The deterministic forecast is the mean of the simulated one wherever a mean is additive: along a
//! single chain of finish-to-start constraints, the expected finish is the sum of expected
//! durations, so the sampled mean finish must reproduce the CPM finish.

use super::*;

fn at(hours: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::UNIX_EPOCH + chrono::TimeDelta::hours(hours)
}

/// Twelve estimated tasks S-0 .. S-11 in one finish-to-start chain without lag.
fn chain() -> Plan {
    let mut plan = layered(12);
    let order: Vec<_> = (0..12)
        .map(|n| {
            let key = dpm_model::Key::new(format!("S-{n}"));
            plan.work_items
                .values()
                .find(|w| w.key == key)
                .expect("layered task")
                .id
        })
        .collect();
    plan.dependencies = order
        .windows(2)
        .map(|pair| {
            dpm_model::Dependency::new(
                pair[0],
                pair[1],
                dpm_model::DependencyKind::FinishStart,
                0.0,
            )
        })
        .collect();
    plan
}

fn task_mut<'a>(plan: &'a mut Plan, key: &str) -> &'a mut dpm_model::WorkItem {
    let key = dpm_model::Key::new(key);
    plan.work_items
        .values_mut()
        .find(|w| w.key == key)
        .expect("chain task")
}

fn mean_sampled_finish(plan: &Plan, sources: &[(WorkItemId, RemainingDuration)]) -> f64 {
    let config = SimulationConfig {
        iterations: 20_000,
        seed: 40,
    };
    let (finishes, _) = sampled_projections(plan, config, sources);
    finishes.iter().sum::<f64>() / finishes.len() as f64
}

#[test]
fn baseline_mean_finish_equals_the_pert_critical_path() {
    let plan = chain();
    let expected = crate::deterministic(&plan)
        .expect("cpm")
        .project_finish_hours;
    let mean = mean_sampled_finish(&plan, &baseline(&plan));
    assert!(
        (mean - expected).abs() < 0.01 * expected,
        "{mean} vs {expected}"
    );
}

#[test]
fn remaining_mean_finish_equals_the_remaining_critical_path_for_started_work() {
    let mut plan = chain();
    let first = task_mut(&mut plan, "S-0");
    first.execution.status = WorkStatus::Verified;
    first.execution.owner = Some(ActorId::agent("owner"));
    first.execution.events.started_at = Some(at(0));
    first.execution.events.verified_at = Some(at(2));
    // S-1 is 1/2/6 h (PERT 2.5 h) and has run for 3 h, past its expectation.
    let second = task_mut(&mut plan, "S-1");
    second.execution.status = WorkStatus::InProgress;
    second.execution.owner = Some(ActorId::agent("owner"));
    second.execution.events.started_at = Some(at(2));
    let now = at(5);
    let timeline = dpm_model::Timeline::at(&plan, now);
    let remaining = crate::cpm::remaining_plan_at(&plan, &timeline);
    let expected = crate::deterministic_remaining(&plan, now)
        .expect("cpm")
        .project_finish_hours;
    let mean = mean_sampled_finish(&remaining.plan, &remaining.durations().expect("durations"));
    assert!(
        (mean - expected).abs() < 0.01 * expected,
        "{mean} vs {expected}"
    );
    let untouched = crate::deterministic(&chain())
        .expect("cpm")
        .project_finish_hours;
    // S-0 is 0.5/1/3 h (PERT 1.25 h); what follows S-1 is the rest of the chain.
    let first_two = 1.25 + 2.5;
    assert!(
        expected > untouched - first_two,
        "S-1 keeps a positive remainder although its PERT expectation has passed"
    );
}
