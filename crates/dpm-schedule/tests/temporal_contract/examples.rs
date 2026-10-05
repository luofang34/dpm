//! Hand-computed worked examples; the same numbers appear in `docs/architecture.md`.

use super::{KINDS, Network, TOLERANCE, stable_id};
use dpm_model::{DependencyKind, WorkItemId};
use dpm_schedule::{
    Schedule, ScheduleError, SimulationConfig, deterministic, deterministic_remaining, simulate,
};

/// Expected `(ES, EF, LS, LF, total float, free float)` in hours.
type Bounds = (f64, f64, f64, f64, f64, f64);

fn assert_bounds(schedule: &Schedule, id: WorkItemId, expected: Bounds, label: &str) {
    let a = &schedule.activities[&id];
    let actual = (
        a.earliest_start_hours,
        a.earliest_finish_hours,
        a.latest_start_hours,
        a.latest_finish_hours,
        a.total_float_hours,
        a.free_float_hours,
    );
    let pairs = [
        (actual.0, expected.0),
        (actual.1, expected.1),
        (actual.2, expected.2),
        (actual.3, expected.3),
        (actual.4, expected.4),
        (actual.5, expected.5),
    ];
    assert!(
        pairs.iter().all(|(x, y)| (x - y).abs() <= TOLERANCE),
        "{label}: expected {expected:?}, got {actual:?}"
    );
    assert_eq!(a.critical, expected.4 <= TOLERANCE, "{label}: criticality");
    assert_eq!(
        schedule.critical_activities.contains(&id),
        a.critical,
        "{label}: critical list"
    );
}

/// A (4h) -> B (3h) under one relation and lag, beside an independent 10h activity C:
/// `(relation, lag, project finish, A bounds, B bounds)`.
#[rustfmt::skip]
const WORKED: [(DependencyKind, f64, f64, Bounds, Bounds); 10] = {
    use DependencyKind::{FinishFinish as FF, FinishStart as FS, StartFinish as SF, StartStart as SS};
    [
        (FS,  2.0, 10.0, (0., 4., 1., 5., 1., 0.),  (6., 9., 7., 10., 1., 1.)),
        (FS, -2.0, 10.0, (0., 4., 5., 9., 5., 0.),  (2., 5., 7., 10., 5., 5.)),
        (FS,  4.0, 11.0, (0., 4., 0., 4., 0., 0.),  (8., 11., 8., 11., 0., 0.)),
        (SS,  2.0, 10.0, (0., 4., 5., 9., 5., 0.),  (2., 5., 7., 10., 5., 5.)),
        (SS, -1.0, 10.0, (0., 4., 6., 10., 6., 1.), (0., 3., 7., 10., 7., 7.)),
        (FF,  2.0, 10.0, (0., 4., 4., 8., 4., 0.),  (3., 6., 7., 10., 4., 4.)),
        (FF, -3.0, 10.0, (0., 4., 6., 10., 6., 2.), (0., 3., 7., 10., 7., 7.)),
        (SF,  5.0, 10.0, (0., 4., 5., 9., 5., 0.),  (2., 5., 7., 10., 5., 5.)),
        (SF,  1.0, 10.0, (0., 4., 6., 10., 6., 2.), (0., 3., 7., 10., 7., 7.)),
        (SF,  9.0, 10.0, (0., 4., 1., 5., 1., 0.),  (6., 9., 7., 10., 1., 1.)),
    ]
};

#[test]
fn worked_examples_cover_every_relation_with_lag_and_lead() {
    for (kind, lag, finish, a_bounds, b_bounds) in WORKED {
        let mut net = Network::new();
        let a = net.task(stable_id(1), 4.0);
        let b = net.task(stable_id(2), 3.0);
        let c = net.task(stable_id(3), 10.0);
        net.link(a, b, kind, lag);
        let before = net.plan.clone();
        let schedule = deterministic(&net.plan).expect("schedule");
        let label = format!("{kind:?} lag {lag}");
        assert_eq!(schedule.project_finish_hours, finish, "{label}");
        assert_bounds(&schedule, a, a_bounds, &format!("{label} A"));
        assert_bounds(&schedule, b, b_bounds, &format!("{label} B"));
        let slack = finish - 10.0;
        let c_bounds = (0., 10., slack, finish, slack, slack);
        assert_bounds(&schedule, c, c_bounds, &format!("{label} C"));
        assert_eq!(
            net.plan, before,
            "{label}: projection must not edit the plan"
        );
        // A lead that would start B before the origin leaves the relation slack, not driving.
        let bound = match kind {
            DependencyKind::FinishStart => 4.0 + lag,
            DependencyKind::StartStart => lag,
            DependencyKind::FinishFinish => 4.0 - 3.0 + lag,
            DependencyKind::StartFinish => lag - 3.0,
        };
        let relation = schedule.relations[&net.plan.dependencies[0].id];
        let slack = b_bounds.0 - bound;
        assert!(
            (relation.slack_hours - slack).abs() <= TOLERANCE,
            "{label}: slack"
        );
        assert_eq!(relation.driving, slack <= TOLERANCE, "{label}: driving");
        let both = a_bounds.4 <= TOLERANCE && b_bounds.4 <= TOLERANCE;
        assert_eq!(
            relation.critical,
            relation.driving && both,
            "{label}: critical"
        );
    }
}

#[test]
fn zero_duration_milestones_carry_bounds_through_a_chain() {
    let mut net = Network::new();
    let a = net.task(stable_id(1), 4.0);
    let m = net.milestone(stable_id(2));
    let b = net.task(stable_id(3), 3.0);
    let c = net.task(stable_id(4), 10.0);
    net.link(a, m, DependencyKind::FinishStart, 0.0);
    net.link(m, b, DependencyKind::FinishStart, 1.0);
    let schedule = deterministic(&net.plan).expect("schedule");
    assert_eq!(schedule.project_finish_hours, 10.0);
    assert_bounds(&schedule, a, (0., 4., 2., 6., 2., 0.), "A");
    assert_bounds(&schedule, m, (4., 4., 6., 6., 2., 0.), "M");
    assert_bounds(&schedule, b, (5., 8., 7., 10., 2., 2.), "B");
    assert_bounds(&schedule, c, (0., 10., 0., 10., 0., 0.), "C");
}

#[test]
fn milestone_joining_parallel_branches_waits_for_the_longest_branch() {
    let mut net = Network::new();
    let a = net.task(stable_id(1), 4.0);
    let b = net.task(stable_id(2), 6.0);
    let m = net.milestone(stable_id(3));
    let d = net.task(stable_id(4), 2.0);
    net.link(a, m, DependencyKind::FinishStart, 0.0);
    net.link(b, m, DependencyKind::FinishStart, 0.0);
    net.link(m, d, DependencyKind::FinishStart, 0.0);
    let schedule = deterministic(&net.plan).expect("schedule");
    assert_eq!(schedule.project_finish_hours, 8.0);
    assert_bounds(&schedule, a, (0., 4., 2., 6., 2., 2.), "A");
    assert_bounds(&schedule, b, (0., 6., 0., 6., 0., 0.), "B");
    assert_bounds(&schedule, m, (6., 6., 6., 6., 0., 0.), "M");
    assert_bounds(&schedule, d, (6., 8., 6., 8., 0., 0.), "D");
    assert_eq!(schedule.critical_activities.len(), 3);
}

/// A zero-duration endpoint has coincident start and finish, so relations that differ only at
/// that endpoint must produce the same bounds.
#[test]
fn zero_duration_endpoints_make_start_and_finish_relations_coincide() {
    let successor_start = |kind: DependencyKind, milestone_first: bool| {
        let mut net = Network::new();
        let a = net.task(stable_id(1), 4.0);
        let m = net.milestone(stable_id(2));
        let b = net.task(stable_id(3), 3.0);
        net.link(a, m, DependencyKind::FinishStart, 0.0);
        let (from, to, probe) = if milestone_first {
            (m, b, b)
        } else {
            (a, m, m)
        };
        if !milestone_first {
            net.plan.dependencies.clear();
        }
        net.link(from, to, kind, 1.0);
        deterministic(&net.plan).expect("schedule").activities[&probe].earliest_start_hours
    };
    use DependencyKind::{
        FinishFinish as FF, FinishStart as FS, StartFinish as SF, StartStart as SS,
    };
    // Milestone predecessor at 4h: finish == start, so FS == SS and FF == SF.
    assert_eq!(successor_start(FS, true), 5.0);
    assert_eq!(successor_start(SS, true), 5.0);
    assert_eq!(successor_start(FF, true), 2.0);
    assert_eq!(successor_start(SF, true), 2.0);
    // Milestone successor of a 4h task: its finish is its start, so FS == FF and SS == SF.
    assert_eq!(successor_start(FS, false), 5.0);
    assert_eq!(successor_start(FF, false), 5.0);
    assert_eq!(successor_start(SS, false), 1.0);
    assert_eq!(successor_start(SF, false), 1.0);
}

#[test]
fn all_zero_duration_plans_finish_at_accumulated_lag() {
    let mut net = Network::new();
    let first = net.milestone(stable_id(1));
    let second = net.milestone(stable_id(2));
    let schedule = deterministic(&net.plan).expect("empty milestones");
    assert_eq!(schedule.project_finish_hours, 0.0);
    assert_eq!(schedule.critical_activities.len(), 2);
    net.link(first, second, DependencyKind::StartStart, 5.0);
    let schedule = deterministic(&net.plan).expect("lagged milestones");
    assert_eq!(schedule.project_finish_hours, 5.0);
    assert_bounds(&schedule, first, (0., 0., 0., 0., 0., 0.), "first");
    assert_bounds(&schedule, second, (5., 5., 5., 5., 0., 0.), "second");
}

fn assert_cycle_rejected(net: &Network, label: &str) {
    let before = net.plan.clone();
    assert!(
        matches!(
            deterministic(&net.plan),
            Err(ScheduleError::DependencyCycle)
        ),
        "{label}: baseline"
    );
    assert!(
        matches!(
            deterministic_remaining(&net.plan, chrono::DateTime::UNIX_EPOCH),
            Err(ScheduleError::Validation(_))
        ),
        "{label}: remaining forecast validates the plan first"
    );
    let config = SimulationConfig {
        iterations: 4,
        seed: 1,
    };
    assert!(
        matches!(
            simulate(&net.plan, config),
            Err(ScheduleError::Validation(_))
        ),
        "{label}: simulation validates the plan first"
    );
    assert_eq!(
        net.plan, before,
        "{label}: rejection must not edit the plan"
    );
}

#[test]
fn every_directed_cycle_is_rejected_regardless_of_relation_or_lead() {
    for kind in KINDS {
        let mut net = Network::new();
        let a = net.task(stable_id(1), 1.0);
        net.link(a, a, kind, 0.0);
        assert_cycle_rejected(&net, &format!("{kind:?} self-loop"));

        let mut net = Network::new();
        let a = net.task(stable_id(1), 2.0);
        let b = net.task(stable_id(2), 2.0);
        // Large leads would make these bounds satisfiable, but the graph must stay acyclic.
        net.link(a, b, kind, -10.0);
        net.link(b, a, kind, -10.0);
        assert_cycle_rejected(&net, &format!("{kind:?} two-cycle with lead"));

        let mut net = Network::new();
        let a = net.task(stable_id(1), 1.0);
        let m = net.milestone(stable_id(2));
        let c = net.task(stable_id(3), 1.0);
        net.link(a, m, DependencyKind::FinishStart, 0.0);
        net.link(m, c, kind, 2.0);
        net.link(c, a, DependencyKind::StartStart, -5.0);
        assert_cycle_rejected(&net, &format!("{kind:?} cycle through milestone"));
    }
}
