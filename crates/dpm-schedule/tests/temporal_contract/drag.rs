//! Drag and the mean critical path for each constraint kind, against values computed by hand.

use super::{Network, TOLERANCE, stable_id};
use dpm_model::{DependencyKind, WorkItemId};
use dpm_schedule::{deterministic, drag, pert_approximation};
use std::collections::{BTreeMap, BTreeSet};

fn drags(net: &Network, ids: &[WorkItemId]) -> BTreeMap<WorkItemId, f64> {
    drag(&net.plan, &ids.iter().copied().collect::<BTreeSet<_>>()).expect("drag")
}

fn assert_hours(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= TOLERANCE,
        "{what}: expected {expected}, got {actual}"
    );
}

/// A(10) -FS-> C(5) and B(4) -FS-> C: finish 15, B floats 6 h.
/// Without A, C starts after B at 4 and the finish is 9, so A's drag is min(10, 6) = 6 h.
/// Without C the finish is A's 10, so C's drag is its whole 5 h. B is off the critical path.
#[test]
fn finish_start_drag_is_limited_by_the_parallel_path_float() {
    let mut net = Network::new();
    let a = net.task(stable_id(1), 10.0);
    let b = net.task(stable_id(2), 4.0);
    let c = net.task(stable_id(3), 5.0);
    net.link(a, c, DependencyKind::FinishStart, 0.0);
    net.link(b, c, DependencyKind::FinishStart, 0.0);
    let schedule = deterministic(&net.plan).expect("schedule");
    assert_hours(schedule.activities[&b].total_float_hours, 6.0, "B float");
    let drag = drags(&net, &[a, b, c]);
    assert_hours(drag[&a], 6.0, "A drag");
    assert_hours(drag[&b], 0.0, "B drag");
    assert_hours(drag[&c], 5.0, "C drag");
}

/// A(10) -SS+2-> B(5): B runs 2..7 inside A, so A alone sets the finish of 10.
/// Without A the finish is B's 7, a drag of 3 h; B floats 3 h and has none.
#[test]
fn start_start_drag_stops_at_the_overlapping_successor() {
    let mut net = Network::new();
    let a = net.task(stable_id(1), 10.0);
    let b = net.task(stable_id(2), 5.0);
    net.link(a, b, DependencyKind::StartStart, 2.0);
    let drag = drags(&net, &[a, b]);
    assert_hours(drag[&a], 3.0, "A drag");
    assert_hours(drag[&b], 0.0, "B drag");
}

/// A(10) -FF-> B(3): B must finish no earlier than A, so it runs 7..10 and both are critical.
/// Without A, B runs 0..3 and the finish is 3: A's drag is 7 h. Without B its finish is still
/// held at A's 10, so B is critical with no drag.
#[test]
fn finish_finish_successor_is_critical_without_drag() {
    let mut net = Network::new();
    let a = net.task(stable_id(1), 10.0);
    let b = net.task(stable_id(2), 3.0);
    net.link(a, b, DependencyKind::FinishFinish, 0.0);
    let schedule = deterministic(&net.plan).expect("schedule");
    assert!(schedule.activities[&a].critical && schedule.activities[&b].critical);
    let drag = drags(&net, &[a, b]);
    assert_hours(drag[&a], 7.0, "A drag");
    assert_hours(drag[&b], 0.0, "B drag");
}

/// C(3) -FS-> A(2) -SF+10-> B(4): A starts at 3, so B must finish by 13 and runs 9..13.
/// Without C, A starts at 0 and B finishes at 10: C's drag is 3 h. A's duration moves neither its
/// start nor B, and B's finish is pinned at 13, so both are critical with no drag.
#[test]
fn start_finish_drag_comes_from_the_predecessor_start() {
    let mut net = Network::new();
    let c = net.task(stable_id(1), 3.0);
    let a = net.task(stable_id(2), 2.0);
    let b = net.task(stable_id(3), 4.0);
    net.link(c, a, DependencyKind::FinishStart, 0.0);
    net.link(a, b, DependencyKind::StartFinish, 10.0);
    let schedule = deterministic(&net.plan).expect("schedule");
    assert_hours(schedule.project_finish_hours, 13.0, "finish");
    assert!(schedule.activities[&a].critical && schedule.activities[&b].critical);
    let drag = drags(&net, &[a, b, c]);
    assert_hours(drag[&c], 3.0, "C drag");
    assert_hours(drag[&a], 0.0, "A drag");
    assert_hours(drag[&b], 0.0, "B drag");
}

/// A(8/10/12) -SS+2-> B(6/9/12): B finishes at 2 + 9 = 11 after A's 10, and A's duration does
/// not reach B's finish, so the path's variance is B's ((12 - 6) / 6)² = 1 alone.
/// With FF+1 instead, B finishes 1 h after A whatever B takes: the variance is A's (4/6)².
#[test]
fn path_variance_counts_only_durations_the_finish_depends_on() {
    for (kind, lag, finish, variance) in [
        (DependencyKind::StartStart, 2.0, 11.0, 1.0),
        (DependencyKind::FinishFinish, 1.0, 11.0, 4.0 / 9.0),
    ] {
        let mut net = Network::new();
        let a = net.pert_task(stable_id(1), 8.0, 10.0, 12.0);
        let b = net.pert_task(stable_id(2), 6.0, 9.0, 12.0);
        net.link(a, b, kind, lag);
        let approximation = pert_approximation(&net.plan).expect("approximation");
        assert_eq!(approximation.path, [a, b], "{kind:?}");
        assert_hours(approximation.mean_hours, finish, "mean");
        assert_hours(approximation.variance_hours2, variance, "variance");
    }
}
