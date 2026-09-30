use super::*;
use dpm_model::{ActorId, ThreePointEstimate, WorkStatus};

mod agreement;

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
        work.execution.status = WorkStatus::Verified;
        work.execution.owner = Some(ActorId::agent("owner"));
        work.execution.events.verified_at = Some(chrono::DateTime::UNIX_EPOCH);
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
        chrono::DateTime::UNIX_EPOCH + chrono::TimeDelta::hours(5),
    )
    .expect("remaining");
    assert_eq!(result.p95_finish_hours, 0.0);
}

/// `count` estimated tasks, each constrained by up to three of the fifty before it through FS,
/// SS and FF edges with lags, so the network has parallel paths and shifting critical sets.
fn layered(count: usize) -> Plan {
    let base = fixture();
    let template = base
        .work_items
        .values()
        .find(|w| w.is_executable())
        .cloned()
        .expect("task");
    let mut plan = Plan::empty("layered");
    let project = base.projects[&template.project].clone();
    plan.projects.insert(project.id, project);
    let mut ids = Vec::with_capacity(count);
    for i in 0..count {
        let hours = 1.0 + (i % 7) as f64;
        let work = dpm_model::WorkItem {
            id: dpm_model::WorkItemId::new(),
            key: dpm_model::Key::new(format!("S-{i}")),
            parent: None,
            contract: dpm_model::WorkContract {
                requirement_ids: Default::default(),
                assets: Vec::new(),
                ..(template.clone()).contract
            },
            execution: dpm_model::ExecutionRecord {
                status: WorkStatus::Planned,
                owner: None,
                artifact_ids: Default::default(),
                ..(template.clone()).execution
            },
            schedule: dpm_model::ScheduleInputs {
                estimate: Some(ThreePointEstimate {
                    optimistic_hours: hours * 0.5,
                    likely_hours: hours,
                    pessimistic_hours: hours * 3.0,
                }),
                ..(template.clone()).schedule
            },
            ..template.clone()
        };
        ids.push(work.id);
        plan.work_items.insert(work.id, work);
    }
    let kinds = [
        dpm_model::DependencyKind::FinishStart,
        dpm_model::DependencyKind::StartStart,
        dpm_model::DependencyKind::FinishFinish,
    ];
    for (i, successor) in ids.iter().enumerate().skip(1) {
        for step in [1_usize, 17, 49].into_iter().filter(|step| *step <= i) {
            plan.dependencies.push(dpm_model::Dependency::new(
                ids[i - step],
                *successor,
                kinds[(i + step) % 3],
                (step % 3) as f64,
            ));
        }
    }
    plan
}

/// Every activity's baseline duration source: its whole beta-PERT duration.
fn baseline(plan: &Plan) -> Vec<(WorkItemId, RemainingDuration)> {
    plan.work_items
        .iter()
        .map(|(id, work)| {
            (
                *id,
                RemainingDuration::of(*id, work, false, 0.0).expect("valid"),
            )
        })
        .collect()
}

/// Sampled project finishes and critical counts, one public deterministic projection per sample.
fn sampled_projections(
    plan: &Plan,
    config: SimulationConfig,
    sources: &[(WorkItemId, RemainingDuration)],
) -> (Vec<f64>, BTreeMap<WorkItemId, usize>) {
    let mut rng = XorShift64::new(config.seed);
    let mut finishes = Vec::new();
    let mut counts: BTreeMap<_, usize> = plan.work_items.keys().map(|id| (*id, 0)).collect();
    for _ in 0..config.iterations {
        let durations = sources
            .iter()
            .map(|(id, source)| (*id, source.sample(&mut rng)))
            .collect();
        let schedule = crate::deterministic_with_durations(plan, &durations).expect("cpm");
        finishes.push(schedule.project_finish_hours);
        for id in schedule.critical_activities {
            *counts.entry(id).or_default() += 1;
        }
    }
    (finishes, counts)
}

/// The simulation computed step by step through the public deterministic projection.
fn reference(
    plan: &Plan,
    config: SimulationConfig,
    sources: &[(WorkItemId, RemainingDuration)],
) -> SimulationSummary {
    let (mut finishes, counts) = sampled_projections(plan, config, sources);
    finishes.sort_by(f64::total_cmp);
    SimulationSummary {
        iterations: config.iterations,
        p50_finish_hours: percentile(&finishes, 0.50),
        p80_finish_hours: percentile(&finishes, 0.80),
        p95_finish_hours: percentile(&finishes, 0.95),
        criticality: counts
            .into_iter()
            .map(|(id, n)| (id, n as f64 / config.iterations as f64))
            .collect(),
    }
}

#[test]
fn simulation_equals_one_deterministic_projection_per_sample() {
    for plan in [fixture(), layered(120)] {
        let config = SimulationConfig {
            iterations: 300,
            seed: 7,
        };
        let fast = simulate(&plan, config).expect("simulate");
        let slow = reference(&plan, config, &baseline(&plan));
        assert_eq!(fast.p50_finish_hours, slow.p50_finish_hours);
        assert_eq!(fast.p80_finish_hours, slow.p80_finish_hours);
        assert_eq!(fast.p95_finish_hours, slow.p95_finish_hours);
        assert_eq!(fast.criticality, slow.criticality);
    }
}

#[test]
fn simulation_cost_is_linear_in_the_graph_per_iteration() {
    // Scanning every edge for every activity (O(N·E) per sample) takes minutes here; one pass
    // over each activity's own edges takes well under a second even in a debug build.
    let plan = layered(3_000);
    let config = SimulationConfig {
        iterations: 200,
        seed: 11,
    };
    let started = std::time::Instant::now();
    let summary = simulate(&plan, config).expect("simulate");
    let elapsed = started.elapsed();
    assert!(summary.p50_finish_hours > 0.0);
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "3000 activities x 200 samples took {elapsed:?}"
    );
}

#[test]
fn remaining_simulation_equals_one_projection_of_the_remaining_graph_per_sample() {
    let mut plan = layered(80);
    let at = |hours| chrono::DateTime::UNIX_EPOCH + chrono::TimeDelta::hours(hours);
    for n in 0..20 {
        let key = dpm_model::Key::new(format!("S-{n}"));
        let work = plan
            .work_items
            .values_mut()
            .find(|w| w.key == key)
            .expect("layered task");
        work.execution.status = WorkStatus::Verified;
        work.execution.owner = Some(ActorId::agent("owner"));
        work.execution.events.started_at = Some(at(n));
        work.execution.events.verified_at = Some(at(n + 1));
    }
    // In flight: not yet started at the first clock reading, mid-estimate at the second and past
    // the pessimistic bound at the third.
    for (n, started) in [(20, 20), (21, 23)] {
        let key = dpm_model::Key::new(format!("S-{n}"));
        let work = plan
            .work_items
            .values_mut()
            .find(|w| w.key == key)
            .expect("layered task");
        work.execution.status = WorkStatus::InProgress;
        work.execution.owner = Some(ActorId::agent("owner"));
        work.execution.events.started_at = Some(at(started));
    }
    let config = SimulationConfig {
        iterations: 200,
        seed: 3,
    };
    for now in [at(10), at(25), at(400)] {
        let fast = simulate_remaining(&plan, config, now).expect("remaining");
        let remaining = crate::cpm::remaining_plan_at(&plan, &dpm_model::Timeline::at(&plan, now))
            .expect("remaining");
        assert!(remaining.excluded.is_empty());
        let slow = reference(
            &remaining.plan,
            config,
            &remaining.durations().expect("durations"),
        );
        assert_eq!(fast.p50_finish_hours, slow.p50_finish_hours);
        assert_eq!(fast.p95_finish_hours, slow.p95_finish_hours);
        assert_eq!(fast.criticality, slow.criticality);
    }
}
