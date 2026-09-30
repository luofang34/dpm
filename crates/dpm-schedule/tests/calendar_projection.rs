//! Remaining projections on working calendars, with fixed expected times in Europe/Berlin.
#![cfg(test)]

#[path = "calendar_projection/review_delay.rs"]
mod review_delay;

use chrono::{DateTime, TimeZone, Utc};
use dpm_model::Timeline;
use dpm_model::{
    AcceptanceCriterion, ActorId, ActorKind, Calendars, Dependency, DependencyKind, Key, LagBasis,
    Plan, Priority, Project, ProjectId, ThreePointEstimate, WorkItem, WorkItemId, WorkKind,
    WorkStatus,
};
use dpm_schedule::{
    RemainingOptions, SimulationConfig, deterministic_remaining, deterministic_remaining_with,
    simulate_remaining,
};
use std::collections::BTreeSet;

/// Local Berlin time in March 2026 (CET, before the daylight-saving change).
fn berlin(day: u32, hour: u32) -> DateTime<Utc> {
    chrono_tz::Europe::Berlin
        .with_ymd_and_hms(2026, 3, day, hour, 0, 0)
        .single()
        .expect("local time")
        .with_timezone(&Utc)
}

fn id(n: u64) -> WorkItemId {
    serde_json::from_str(&format!("\"00000000-0000-4000-8000-{n:012x}\"")).expect("id")
}

struct Builder {
    plan: Plan,
    project: ProjectId,
}

impl Builder {
    fn new(calendars: Option<Calendars>) -> Self {
        let mut plan = Plan::empty("calendar projection");
        let project = Project {
            id: ProjectId::new(),
            key: Key::new("P"),
            parent: None,
            title: "Calendars".into(),
            objective: "Place work on calendars".into(),
        };
        let project_id = project.id;
        plan.projects.insert(project_id, project);
        plan.calendars = calendars;
        Self {
            plan,
            project: project_id,
        }
    }

    fn work(
        &mut self,
        n: u64,
        kind: WorkKind,
        hours: f64,
        executor: Option<ActorKind>,
    ) -> WorkItemId {
        let key = format!("W{n}");
        let work = WorkItem {
            id: id(n),
            key: Key::new(key.clone()),
            project: self.project,
            parent: None,
            kind,
            title: key.clone(),
            order: Default::default(),
            contract: dpm_model::WorkContract {
                objective: key,
                acceptance: vec![AcceptanceCriterion {
                    text: "observable".into(),
                }],
                instructions: None,
                capabilities: BTreeSet::new(),
                requirement_ids: BTreeSet::new(),
                condition: None,
                join: dpm_model::JoinPolicy::default(),
                assets: Vec::new(),
            },
            execution: dpm_model::ExecutionRecord {
                status: WorkStatus::Planned,
                reported_progress_percent: 0,
                artifact_ids: BTreeSet::new(),
                owner: None,
                handoffs: Vec::new(),
                releases: Vec::new(),
                block_reason: None,
                events: Default::default(),
                last_rejection: None,
                attempts: Vec::new(),
                basis: Vec::new(),
            },
            schedule: dpm_model::ScheduleInputs {
                priority: Priority::P2,
                estimate: (kind == WorkKind::Task).then_some(ThreePointEstimate {
                    optimistic_hours: hours,
                    likely_hours: hours,
                    pessimistic_hours: hours,
                }),
                executor,
                calendar: None,
            },
        };
        self.plan.work_items.insert(work.id, work);
        id(n)
    }

    fn task(&mut self, n: u64, hours: f64, executor: ActorKind) -> WorkItemId {
        self.work(n, WorkKind::Task, hours, Some(executor))
    }

    fn link(
        &mut self,
        from: WorkItemId,
        to: WorkItemId,
        kind: DependencyKind,
        lag: f64,
        basis: LagBasis,
    ) {
        let mut edge = Dependency::new(from, to, kind, lag);
        edge.lag_basis = basis;
        self.plan.dependencies.push(edge);
    }
}

fn zone() -> Option<Calendars> {
    Some(Calendars::in_zone("Europe/Berlin"))
}

fn hours(now: DateTime<Utc>, at: DateTime<Utc>) -> f64 {
    (at - now).num_milliseconds() as f64 / 3_600_000.0
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
}

#[test]
fn human_work_follows_the_standard_calendar() {
    let now = berlin(2, 0);
    let mut plan = Builder::new(zone());
    let (a, b, c) = (
        plan.task(1, 8.0, ActorKind::Human),
        plan.task(2, 4.0, ActorKind::Human),
        plan.task(3, 4.0, ActorKind::Human),
    );
    plan.link(a, b, DependencyKind::FinishStart, 0.0, LagBasis::Elapsed);
    let schedule = deterministic_remaining(&plan.plan, now).expect("schedule");
    let (a, b, c) = (
        &schedule.activities[&a],
        &schedule.activities[&b],
        &schedule.activities[&c],
    );
    close(a.earliest_start_hours, hours(now, berlin(2, 8)));
    close(a.earliest_finish_hours, hours(now, berlin(2, 17)));
    close(b.earliest_start_hours, hours(now, berlin(3, 8)));
    close(b.earliest_finish_hours, hours(now, berlin(3, 12)));
    close(schedule.project_finish_hours, hours(now, berlin(3, 12)));
    close(c.earliest_finish_hours, hours(now, berlin(2, 12)));
    close(c.total_float_hours, hours(berlin(2, 8), berlin(3, 8)));
    let calendar = a.calendar.as_ref().expect("calendar report");
    assert_eq!(calendar.resolved.calendar, "standard");
    assert_eq!(calendar.resolved.executor, ActorKind::Human);
    close(calendar.review_wait_hours, 0.0);
}

#[test]
fn agent_work_runs_around_the_clock_and_waits_for_human_review() {
    let now = berlin(6, 12);
    let mut plan = Builder::new(zone());
    let agent = plan.task(1, 20.0, ActorKind::Agent);
    let schedule = deterministic_remaining(&plan.plan, now).expect("schedule");
    let activity = &schedule.activities[&agent];
    close(activity.earliest_start_hours, 0.0);
    close(activity.earliest_finish_hours, hours(now, berlin(9, 8)));
    let calendar = activity.calendar.as_ref().expect("calendar report");
    close(
        calendar.review_wait_hours,
        hours(berlin(7, 8), berlin(9, 8)),
    );
    let mut agents_review = plan.plan.clone();
    if let Some(calendars) = agents_review.calendars.as_mut() {
        calendars.verifier = ActorKind::Agent;
    }
    let schedule = deterministic_remaining(&agents_review, now).expect("schedule");
    close(schedule.project_finish_hours, 20.0);
}

#[test]
fn working_lags_count_the_successors_calendar() {
    let now = berlin(2, 0);
    for (basis, start, finish) in [
        (LagBasis::Working, berlin(4, 8), berlin(4, 17)),
        (LagBasis::Elapsed, berlin(3, 8), berlin(3, 17)),
    ] {
        let mut plan = Builder::new(zone());
        let (a, b) = (
            plan.task(1, 8.0, ActorKind::Human),
            plan.task(2, 8.0, ActorKind::Human),
        );
        plan.link(a, b, DependencyKind::FinishStart, 8.0, basis);
        let schedule = deterministic_remaining(&plan.plan, now).expect("schedule");
        close(
            schedule.activities[&b].earliest_start_hours,
            hours(now, start),
        );
        close(
            schedule.activities[&b].earliest_finish_hours,
            hours(now, finish),
        );
        close(schedule.activities[&a].total_float_hours, 0.0);
    }
}

#[test]
fn finish_constraints_move_the_start_back_on_the_calendar() {
    let now = berlin(2, 0);
    let mut plan = Builder::new(zone());
    let (a, b) = (
        plan.task(1, 8.0, ActorKind::Human),
        plan.task(2, 4.0, ActorKind::Human),
    );
    plan.link(a, b, DependencyKind::FinishFinish, 0.0, LagBasis::Elapsed);
    let schedule = deterministic_remaining(&plan.plan, now).expect("schedule");
    close(
        schedule.activities[&b].earliest_start_hours,
        hours(now, berlin(2, 13)),
    );
    close(
        schedule.activities[&b].earliest_finish_hours,
        hours(now, berlin(2, 17)),
    );
}

#[test]
fn started_work_counts_working_hours_since_its_start() {
    let now = berlin(3, 8);
    let mut plan = Builder::new(zone());
    let task = plan.task(1, 16.0, ActorKind::Human);
    let work = plan.plan.work_items.get_mut(&task).expect("task");
    work.execution.status = WorkStatus::InProgress;
    work.execution.owner = Some(ActorId::human("ada"));
    work.execution.events.started_at = Some(berlin(2, 8));
    let schedule = deterministic_remaining(&plan.plan, now).expect("schedule");
    close(
        schedule.activities[&task].earliest_finish_hours,
        hours(now, berlin(3, 17)),
    );
}

#[test]
fn always_calendars_reproduce_the_elapsed_projection() {
    let now = berlin(6, 12);
    let mut elapsed = Builder::new(None);
    let mut always = Builder::new(Some(Calendars {
        default_executor: ActorKind::Agent,
        verifier: ActorKind::Agent,
        ..Calendars::in_zone("Europe/Berlin")
    }));
    for plan in [&mut elapsed, &mut always] {
        let a = plan.work(1, WorkKind::Task, 7.5, None);
        let b = plan.work(2, WorkKind::Task, 3.0, None);
        let c = plan.work(3, WorkKind::Task, 11.0, None);
        let m = plan.work(4, WorkKind::Milestone, 0.0, None);
        plan.link(a, b, DependencyKind::StartStart, 2.0, LagBasis::Working);
        plan.link(a, c, DependencyKind::FinishFinish, -1.0, LagBasis::Elapsed);
        plan.link(b, m, DependencyKind::FinishStart, 0.5, LagBasis::Working);
        plan.link(c, m, DependencyKind::StartFinish, 4.0, LagBasis::Elapsed);
    }
    let reference = deterministic_remaining(&elapsed.plan, now).expect("elapsed");
    let placed = deterministic_remaining(&always.plan, now).expect("always");
    close(placed.project_finish_hours, reference.project_finish_hours);
    for (id, expected) in &reference.activities {
        let actual = &placed.activities[id];
        for (x, y) in [
            (actual.earliest_start_hours, expected.earliest_start_hours),
            (actual.earliest_finish_hours, expected.earliest_finish_hours),
            (actual.latest_start_hours, expected.latest_start_hours),
            (actual.latest_finish_hours, expected.latest_finish_hours),
            (actual.total_float_hours, expected.total_float_hours),
            (actual.free_float_hours, expected.free_float_hours),
        ] {
            close(x, y);
        }
        assert_eq!(actual.critical, expected.critical);
    }
}

#[test]
fn simulation_places_samples_on_the_same_calendars() {
    let now = berlin(2, 0);
    let mut plan = Builder::new(zone());
    let (a, b) = (
        plan.task(1, 8.0, ActorKind::Human),
        plan.task(2, 4.0, ActorKind::Human),
    );
    plan.link(a, b, DependencyKind::FinishStart, 0.0, LagBasis::Elapsed);
    let config = SimulationConfig {
        iterations: 200,
        seed: 7,
    };
    let simulation = simulate_remaining(&plan.plan, config, now).expect("simulation");
    close(simulation.p50_finish_hours, hours(now, berlin(3, 12)));
    close(simulation.p95_finish_hours, hours(now, berlin(3, 12)));
    let long = Builder::new(zone());
    let mut long = long;
    long.task(1, 2000.0, ActorKind::Human);
    let simulation = simulate_remaining(&long.plan, config, now).expect("wide window");
    assert!(simulation.p50_finish_hours > 24.0 * 7.0 * 49.0);
}

#[test]
fn clock_readings_with_milliseconds_do_not_invent_review_waits() {
    for step in 0..40_i64 {
        let now = berlin(2, 0) + chrono::TimeDelta::milliseconds(37_000 * step + 1);
        let mut plan = Builder::new(zone());
        let task = plan.task(1, 8.0, ActorKind::Human);
        let schedule = deterministic_remaining(&plan.plan, now).expect("schedule");
        let activity = &schedule.activities[&task];
        close(activity.earliest_finish_hours, hours(now, berlin(2, 17)));
        let calendar = activity.calendar.as_ref().expect("calendar");
        close(calendar.review_wait_hours, 0.0);
    }
}

#[test]
fn a_start_finish_edge_from_agent_work_keeps_latest_after_earliest() {
    let now = Utc
        .with_ymd_and_hms(2026, 3, 2, 0, 1, 0)
        .single()
        .expect("now");
    let mut plan = Builder::new(zone());
    let agent = plan.task(1, 0.0, ActorKind::Agent);
    let human = plan.task(2, 13.0, ActorKind::Human);
    plan.link(
        agent,
        human,
        DependencyKind::StartFinish,
        0.0,
        LagBasis::Elapsed,
    );
    let schedule = deterministic_remaining(&plan.plan, now).expect("schedule");
    for activity in schedule.activities.values() {
        assert!(activity.latest_start_hours >= activity.earliest_start_hours - 1e-6);
    }
}

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Random small networks mixing executor kinds, relation kinds, signed lags, lag bases and review
/// delays, with and without calendars: the latest times never precede the earliest ones, so float
/// is never negative before clamping.
#[test]
fn random_calendar_networks_keep_latest_times_after_earliest() {
    let kinds = [
        DependencyKind::FinishStart,
        DependencyKind::StartStart,
        DependencyKind::FinishFinish,
        DependencyKind::StartFinish,
    ];
    let executors = [ActorKind::Human, ActorKind::Agent, ActorKind::Service];
    let mut random = Random(0x5EED_CA1E_2026);
    for case in 0..1500 {
        let now =
            berlin(2, 0) + chrono::TimeDelta::milliseconds((random.below(7 * 86_400) * 997) as i64);
        let verifier = executors[random.below(3) as usize];
        let calendars = (case % 5 != 0).then(|| Calendars {
            verifier,
            ..Calendars::in_zone("Europe/Berlin")
        });
        let mut plan = Builder::new(calendars);
        let options = RemainingOptions {
            review_delay_hours: random.below(4) as f64 * 3.5,
        };
        let count = 2 + random.below(6);
        let ids: Vec<WorkItemId> = (0..count)
            .map(|n| {
                let executor = executors[random.below(3) as usize];
                plan.task(n + 1, random.below(17) as f64 * 0.75, executor)
            })
            .collect();
        for (to, successor) in ids.iter().enumerate().skip(1) {
            let from = random.below(to as u64) as usize;
            let lag = random.below(16) as f64 - 6.0;
            let basis = if random.below(2) == 0 {
                LagBasis::Working
            } else {
                LagBasis::Elapsed
            };
            plan.link(
                ids[from],
                *successor,
                kinds[random.below(4) as usize],
                lag,
                basis,
            );
        }
        let timeline = Timeline::at(&plan.plan, now);
        let schedule =
            deterministic_remaining_with(&plan.plan, &timeline, options).expect("schedule");
        for (id, activity) in &schedule.activities {
            assert!(
                activity.latest_start_hours >= activity.earliest_start_hours - 1e-6
                    && activity.latest_finish_hours >= activity.earliest_finish_hours - 1e-6,
                "case {case} {id} at {now}: {activity:?}\n{:#?}\n{:?}",
                plan.plan
                    .dependencies
                    .iter()
                    .map(|d| (d.predecessor, d.successor, d.kind, d.lag_hours, d.lag_basis))
                    .collect::<Vec<_>>(),
                plan.plan
                    .work_items
                    .values()
                    .map(|w| (
                        w.id,
                        w.schedule.executor,
                        w.schedule.estimate.map(|e| e.likely_hours)
                    ))
                    .collect::<Vec<_>>()
            );
        }
    }
}
