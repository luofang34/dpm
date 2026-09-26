use super::*;
use dpm_model::{
    AcceptanceCriterion, Dependency, Key, Priority, Project, ProjectId, ThreePointEstimate,
    WorkItem, WorkKind, WorkStatus,
};
use std::collections::BTreeSet;

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
        block_reason: None,
        last_rejection: None,
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
    plan.dependencies.push(Dependency {
        predecessor: a.id,
        successor: b.id,
        kind: DependencyKind::FinishStart,
        lag_hours: 0.0,
    });

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
        plan.dependencies.push(Dependency {
            predecessor: pred,
            successor: succ,
            kind: DependencyKind::FinishStart,
            lag_hours: 0.0,
        });
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
        plan.dependencies.push(Dependency {
            predecessor: a,
            successor: b,
            kind,
            lag_hours: lag,
        });
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
    let edge = Dependency {
        predecessor: a,
        successor: b,
        kind: DependencyKind::FinishStart,
        lag_hours: 0.0,
    };
    plan.dependencies = vec![edge.clone(), edge];
    assert_eq!(
        deterministic(&plan)
            .expect("duplicate")
            .project_finish_hours,
        6.0
    );
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
    plan.dependencies.push(Dependency {
        predecessor: a,
        successor: b,
        kind: DependencyKind::FinishStart,
        lag_hours: f64::MAX,
    });
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
fn completed_dependencies_do_not_reintroduce_historical_lag() {
    let (mut plan, a, b) = pair(4.0, 3.0);
    plan.dependencies.push(Dependency {
        predecessor: a,
        successor: b,
        kind: DependencyKind::FinishStart,
        lag_hours: 9.0,
    });
    let item = plan.work_items.get_mut(&a).expect("task");
    item.status = WorkStatus::Verified;
    item.owner = Some(dpm_model::ActorId::agent("owner"));
    assert_eq!(
        deterministic_remaining(&plan)
            .expect("remaining")
            .project_finish_hours,
        3.0
    );
    let item = plan.work_items.get_mut(&b).expect("task");
    item.status = WorkStatus::Verified;
    item.owner = Some(dpm_model::ActorId::agent("owner"));
    assert_eq!(
        deterministic_remaining(&plan)
            .expect("complete")
            .project_finish_hours,
        0.0
    );
    assert_eq!(
        deterministic(&plan).expect("baseline").project_finish_hours,
        16.0
    );
}
