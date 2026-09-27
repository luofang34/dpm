use super::*;
use crate::network::relation_weight;
use dpm_model::{
    AcceptanceCriterion, Dependency, DependencyKind, Key, Priority, Project, ProjectId,
    ThreePointEstimate, WorkItem, WorkKind, WorkStatus,
};
use std::collections::BTreeSet;

fn at(hours: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::UNIX_EPOCH + chrono::TimeDelta::hours(hours)
}

fn verify(work: &mut WorkItem, hours: Option<i64>) {
    work.status = WorkStatus::Verified;
    work.owner = Some(dpm_model::ActorId::agent("owner"));
    work.events.verified_at = hours.map(at);
}

fn task(project: ProjectId, key: &str, hours: f64) -> WorkItem {
    WorkItem {
        id: WorkItemId::new(),
        key: Key::new(key),
        project,
        parent: None,
        kind: WorkKind::Task,
        title: key.to_string(),
        objective: key.to_string(),
        acceptance: vec![AcceptanceCriterion {
            text: "observable result exists".into(),
        }],
        instructions: None,
        status: WorkStatus::Planned,
        reported_progress_percent: 0,
        priority: Priority::P2,
        estimate: Some(ThreePointEstimate {
            optimistic_hours: hours,
            likely_hours: hours,
            pessimistic_hours: hours,
        }),
        capabilities: BTreeSet::new(),
        requirement_ids: BTreeSet::new(),
        artifact_ids: BTreeSet::new(),
        owner: None,
        handoffs: Vec::new(),
        releases: Vec::new(),
        block_reason: None,
        events: Default::default(),
        condition: None,
        join: dpm_model::JoinPolicy::default(),
        last_rejection: None,
        attempts: Vec::new(),
        basis: Vec::new(),
        resources: Vec::new(),
    }
}

#[test]
fn finish_start_chain_has_expected_finish() {
    let mut plan = Plan::empty("test");
    let project = Project {
        id: ProjectId::new(),
        key: Key::new("P"),
        parent: None,
        title: "Project".into(),
        objective: "Test scheduling".into(),
    };
    let a = task(project.id, "A", 2.0);
    let b = task(project.id, "B", 3.0);
    plan.projects.insert(project.id, project);
    plan.work_items.insert(a.id, a.clone());
    plan.work_items.insert(b.id, b.clone());
    plan.dependencies.push(Dependency::new(
        a.id,
        b.id,
        DependencyKind::FinishStart,
        0.0,
    ));

    let schedule = deterministic(&plan).expect("schedule");
    assert_eq!(schedule.project_finish_hours, 5.0);
    assert_eq!(schedule.activities[&b.id].earliest_start_hours, 2.0);
    assert!(schedule.activities[&a.id].critical);
    assert!(schedule.activities[&b.id].critical);
}

#[test]
fn cycles_are_rejected() {
    let mut plan = Plan::empty("test");
    let project = Project {
        id: ProjectId::new(),
        key: Key::new("P"),
        parent: None,
        title: "Project".into(),
        objective: "Test cycles".into(),
    };
    let a = task(project.id, "A", 1.0);
    let b = task(project.id, "B", 1.0);
    plan.projects.insert(project.id, project);
    plan.work_items.insert(a.id, a.clone());
    plan.work_items.insert(b.id, b.clone());
    for (pred, succ) in [(a.id, b.id), (b.id, a.id)] {
        plan.dependencies.push(Dependency::new(
            pred,
            succ,
            DependencyKind::FinishStart,
            0.0,
        ));
    }
    assert!(matches!(
        deterministic(&plan),
        Err(ScheduleError::DependencyCycle)
    ));
}

fn pair(a_hours: f64, b_hours: f64) -> (Plan, WorkItemId, WorkItemId) {
    let mut plan = Plan::empty("test");
    let project = Project {
        id: ProjectId::new(),
        key: Key::new("P"),
        parent: None,
        title: "Project".into(),
        objective: "Schedule".into(),
    };
    let a = task(project.id, "A", a_hours);
    let b = task(project.id, "B", b_hours);
    let ids = (a.id, b.id);
    plan.projects.insert(project.id, project);
    plan.work_items.insert(a.id, a);
    plan.work_items.insert(b.id, b);
    (plan, ids.0, ids.1)
}

#[test]
fn all_temporal_relationships_and_lead_lag_produce_expected_dates() {
    for (kind, lag, start, finish) in [
        (DependencyKind::FinishStart, 2.0, 6.0, 9.0),
        (DependencyKind::FinishStart, -2.0, 2.0, 5.0),
        (DependencyKind::StartStart, 2.0, 2.0, 5.0),
        (DependencyKind::FinishFinish, 2.0, 3.0, 6.0),
        (DependencyKind::StartFinish, 5.0, 2.0, 5.0),
    ] {
        let (mut plan, a, b) = pair(4.0, 3.0);
        plan.dependencies.push(Dependency::new(a, b, kind, lag));
        let schedule = deterministic(&plan).expect("schedule");
        assert_eq!(
            schedule.activities[&b].earliest_start_hours, start,
            "{kind:?}"
        );
        assert_eq!(schedule.project_finish_hours, finish, "{kind:?}");
    }
}

#[test]
fn parallel_paths_float_and_duplicate_constraints_are_consistent() {
    let (mut plan, a, b) = pair(4.0, 2.0);
    let schedule = deterministic(&plan).expect("parallel");
    assert_eq!(schedule.activities[&b].total_float_hours, 2.0);
    assert!(schedule.activities[&a].critical);
    assert!(!schedule.activities[&b].critical);
    let edge = Dependency::new(a, b, DependencyKind::FinishStart, 0.0);
    plan.dependencies = vec![edge.clone(), edge];
    assert!(matches!(
        deterministic(&plan),
        Err(ScheduleError::Validation(_))
    ));
    // Distinct relations between the same ordered pair each bound the successor.
    plan.dependencies = vec![
        Dependency::new(a, b, DependencyKind::FinishStart, 0.0),
        Dependency::new(a, b, DependencyKind::StartStart, 5.0),
    ];
    let schedule = deterministic(&plan).expect("parallel relations");
    assert_eq!(schedule.activities[&b].earliest_start_hours, 5.0);
    assert_eq!(schedule.project_finish_hours, 7.0);
}

#[test]
fn missing_negative_nonfinite_and_overflowing_durations_are_rejected() {
    let (mut plan, a, b) = pair(1.0, 1.0);
    for durations in [
        BTreeMap::new(),
        BTreeMap::from([(a, -1.0), (b, 1.0)]),
        BTreeMap::from([(a, f64::NAN), (b, 1.0)]),
    ] {
        assert!(matches!(
            deterministic_with_durations(&plan, &durations),
            Err(ScheduleError::InvalidDuration(_))
        ));
    }
    plan.dependencies
        .push(Dependency::new(a, b, DependencyKind::FinishStart, f64::MAX));
    assert!(matches!(
        deterministic_with_durations(&plan, &BTreeMap::from([(a, f64::MAX), (b, 1.0)])),
        Err(ScheduleError::ArithmeticOverflow(_))
    ));
    plan.dependencies[0].successor = WorkItemId::new();
    assert!(matches!(
        deterministic(&plan),
        Err(ScheduleError::MissingWorkItem(_))
    ));
}

#[test]
fn completed_dependencies_keep_only_the_lag_still_elapsing() {
    let (mut plan, a, b) = pair(4.0, 3.0);
    plan.dependencies
        .push(Dependency::new(a, b, DependencyKind::FinishStart, 9.0));
    verify(plan.work_items.get_mut(&a).expect("task"), Some(0));
    let finish = |plan: &Plan, now| {
        deterministic_remaining(plan, now)
            .expect("remaining")
            .project_finish_hours
    };
    assert_eq!(finish(&plan, at(4)), 5.0 + 3.0, "5h of the 9h lag remain");
    assert_eq!(
        finish(&plan, at(9)),
        3.0,
        "an elapsed lag is not reintroduced"
    );
    verify(plan.work_items.get_mut(&a).expect("task"), None);
    assert_eq!(
        finish(&plan, at(1_000)),
        9.0 + 3.0,
        "a lag from an unrecorded event is never assumed to have elapsed"
    );
    verify(plan.work_items.get_mut(&b).expect("task"), None);
    assert_eq!(finish(&plan, at(1_000)), 0.0);
    assert_eq!(
        deterministic(&plan).expect("baseline").project_finish_hours,
        16.0
    );
}

#[test]
fn free_float_distinguishes_successor_delay_from_project_delay_for_all_relations() {
    for kind in [
        DependencyKind::FinishStart,
        DependencyKind::StartStart,
        DependencyKind::FinishFinish,
        DependencyKind::StartFinish,
    ] {
        for lag in [-5.0, 0.0, 3.0] {
            let (mut plan, a, b) = pair(10.0, 4.0);
            let c = task(plan.work_items[&a].project, "C", 30.0);
            plan.work_items.insert(c.id, c);
            plan.dependencies.push(Dependency::new(a, b, kind, lag));
            let original = plan.clone();
            let schedule = deterministic(&plan).expect("schedule");
            let left = &schedule.activities[&a];
            let right = &schedule.activities[&b];
            assert!(left.free_float_hours <= left.total_float_hours + EPSILON);
            assert!(left.free_float_hours >= 0.0);
            let shifted = left.earliest_start_hours + left.free_float_hours;
            let weight = relation_weight(kind, 10.0, 4.0, lag);
            assert!(shifted + weight <= right.earliest_start_hours + EPSILON);
            assert!(shifted + 10.0 <= schedule.project_finish_hours + EPSILON);
            assert!(
                shifted + 0.1 + weight > right.earliest_start_hours
                    || shifted + 0.1 + 10.0 > schedule.project_finish_hours
            );
            assert_eq!(plan, original);
        }
    }
    let (mut plan, a, b) = pair(10.0, 4.0);
    let c = task(plan.work_items[&a].project, "C", 20.0);
    plan.work_items.insert(c.id, c);
    plan.dependencies
        .push(Dependency::new(a, b, DependencyKind::FinishStart, 0.0));
    let schedule = deterministic(&plan).expect("schedule");
    assert_eq!(schedule.activities[&a].free_float_hours, 0.0);
    assert_eq!(schedule.activities[&a].total_float_hours, 6.0);
    assert_eq!(schedule.activities[&b].free_float_hours, 6.0);
}

#[test]
fn reached_milestones_do_not_reintroduce_historical_lag_in_remaining_forecasts() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let milestone = plan.find_work_by_key("TEST-M1").expect("milestone").id;
    let successor = plan.find_work_by_key("TEST-F").expect("successor").id;
    for task in plan
        .work_items
        .values_mut()
        .filter(|w| w.is_executable() && w.id != successor)
    {
        verify(task, Some(0));
    }
    // The gate keeps an otherwise completed milestone pending until the choice is made.
    let decision = plan.decisions.values_mut().next().expect("gate");
    decision.blocks.insert(milestone);
    plan.dependencies
        .iter_mut()
        .find(|d| d.successor == successor)
        .expect("incoming edge")
        .successor = milestone;
    let outgoing = plan
        .dependencies
        .iter_mut()
        .find(|d| d.predecessor == successor)
        .expect("outgoing edge");
    outgoing.predecessor = milestone;
    outgoing.successor = successor;
    outgoing.lag_hours = 24.0;
    let duration = plan.work_items[&successor].expected_duration_hours();
    assert_eq!(
        deterministic_remaining(&plan, chrono::DateTime::UNIX_EPOCH)
            .expect("pending")
            .project_finish_hours,
        duration + 24.0
    );
    let decision = plan.decisions.values_mut().next().expect("gate");
    decision.status = dpm_model::DecisionStatus::Decided;
    decision.outcome = Some("Acceptance condition met".into());
    decision.resolved_at = Some(at(2));
    let original = plan.clone();
    assert!(dpm_model::completion(&plan, at(2)).contains(&milestone));
    assert_eq!(
        deterministic_remaining(&plan, at(12))
            .expect("elapsing")
            .project_finish_hours,
        duration + 14.0,
        "the milestone was reached when the decision resolved"
    );
    assert_eq!(
        deterministic_remaining(&plan, at(26))
            .expect("remaining")
            .project_finish_hours,
        duration
    );
    let risk = crate::simulate_remaining(
        &plan,
        crate::SimulationConfig {
            iterations: 32,
            seed: 9,
        },
        at(26),
    )
    .expect("simulation");
    let estimate = plan.work_items[&successor].estimate.expect("estimate");
    assert!(risk.p50_finish_hours >= estimate.optimistic_hours);
    assert!(risk.p50_finish_hours <= risk.p80_finish_hours);
    assert!(risk.p80_finish_hours <= risk.p95_finish_hours);
    assert!(risk.p95_finish_hours <= estimate.pessimistic_hours);
    assert_eq!(plan, original);
}

fn start(work: &mut WorkItem, hours: Option<i64>) {
    work.status = WorkStatus::InProgress;
    work.owner = Some(dpm_model::ActorId::agent("owner"));
    work.events.started_at = hours.map(at);
}

#[test]
fn a_started_predecessor_releases_start_based_lag_from_its_start_event() {
    for kind in [DependencyKind::StartStart, DependencyKind::StartFinish] {
        let (mut plan, a, b) = pair(40.0, 3.0);
        plan.dependencies.push(Dependency::new(a, b, kind, 10.0));
        start(plan.work_items.get_mut(&a).expect("task"), Some(0));
        let b_start = |plan: &Plan, now| {
            deterministic_remaining(plan, now)
                .expect("remaining")
                .activities[&b]
                .earliest_start_hours
        };
        // SF bounds B's finish, so B may start its 3h duration before the bound.
        let finish_offset = if kind == DependencyKind::StartFinish {
            3.0
        } else {
            0.0
        };
        assert_eq!(b_start(&plan, at(20)), 0.0, "{kind:?}: lag elapsed at +10h");
        assert_eq!(
            b_start(&plan, at(4)),
            6.0 - finish_offset,
            "{kind:?}: 6h of lag remain"
        );
        start(plan.work_items.get_mut(&a).expect("task"), None);
        assert_eq!(
            b_start(&plan, at(1_000)),
            10.0 - finish_offset,
            "{kind:?}: lag from an unrecorded start is never assumed to have elapsed"
        );
    }
}

#[test]
fn finish_based_lag_from_submitted_but_unverified_work_is_kept_whole() {
    for (kind, expected) in [
        (DependencyKind::FinishStart, 4.0 + 2.0),
        (DependencyKind::FinishFinish, 4.0 + 2.0 - 3.0),
    ] {
        let (mut plan, a, b) = pair(4.0, 3.0);
        plan.dependencies.push(Dependency::new(a, b, kind, 2.0));
        let work = plan.work_items.get_mut(&a).expect("task");
        start(work, Some(0));
        work.status = WorkStatus::Submitted;
        work.events.submitted_at = Some(at(1));
        let schedule = deterministic_remaining(&plan, at(50)).expect("remaining");
        assert_eq!(
            schedule.activities[&b].earliest_start_hours, expected,
            "{kind:?}: only verification finishes the predecessor"
        );
    }
}
