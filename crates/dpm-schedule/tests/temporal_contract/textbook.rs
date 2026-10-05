//! Published PERT/CPM exercises, so the projection is checked against answers computed outside DPM.
//! Each activity is identified by its textbook letter; one textbook week is one elapsed hour here.

use super::{Network, TOLERANCE, stable_id};
use dpm_model::{DependencyKind, WorkItemId};
use dpm_schedule::{Schedule, SimulationConfig, deterministic, simulate};
use std::collections::BTreeMap;

/// `(letter, immediate predecessors, optimistic, most likely, pessimistic)`.
type Activity = (char, &'static str, f64, f64, f64);

fn build(activities: &[Activity]) -> (Network, BTreeMap<char, WorkItemId>) {
    let mut net = Network::new();
    let mut ids = BTreeMap::new();
    for (n, &(letter, predecessors, o, m, p)) in (1u64..).zip(activities) {
        let id = net.pert_task(stable_id(n), o, m, p);
        for predecessor in predecessors.chars() {
            net.link(ids[&predecessor], id, DependencyKind::FinishStart, 0.0);
        }
        ids.insert(letter, id);
    }
    (net, ids)
}

/// Asserts the finish, the critical letters, and every `(letter, ES, LS)`; slack is LS - ES.
fn assert_cpm(
    schedule: &Schedule,
    ids: &BTreeMap<char, WorkItemId>,
    finish: f64,
    critical: &str,
    starts: &[(char, f64, f64)],
) {
    assert!((schedule.project_finish_hours - finish).abs() <= TOLERANCE);
    let flagged: String = ids
        .iter()
        .filter(|(_, id)| schedule.activities[*id].critical)
        .map(|(letter, _)| *letter)
        .collect();
    assert_eq!(flagged, critical);
    for &(letter, es, ls) in starts {
        let a = &schedule.activities[&ids[&letter]];
        assert!(
            (a.earliest_start_hours - es).abs() <= TOLERANCE
                && (a.latest_start_hours - ls).abs() <= TOLERANCE
                && (a.total_float_hours - (ls - es)).abs() <= TOLERANCE,
            "{letter}: expected ES {es} LS {ls}, got {a:?}"
        );
    }
}

/// Hillier & Lieberman, Introduction to Operations Research, ch. 22: the Reliable Construction Co.
/// project (Tables 22.1 and 22.4, slacks in Table 22.3).
const RELIABLE: [Activity; 14] = [
    ('A', "", 1., 2., 3.),
    ('B', "A", 2., 3.5, 8.),
    ('C', "B", 6., 9., 18.),
    ('D', "C", 4., 5.5, 10.),
    ('E', "C", 1., 4.5, 5.),
    ('F', "E", 4., 4., 10.),
    ('G', "D", 5., 6.5, 11.),
    ('H', "EG", 5., 8., 17.),
    ('I', "C", 3., 7.5, 9.),
    ('J', "FI", 3., 9., 9.),
    ('K', "J", 4., 4., 4.),
    ('L', "J", 1., 5.5, 7.),
    ('M', "H", 1., 2., 3.),
    ('N', "KL", 5., 5.5, 9.),
];

#[test]
fn reliable_construction_matches_the_published_schedule_and_slack() {
    let (net, ids) = build(&RELIABLE);
    let schedule = deterministic(&net.plan).expect("schedule");
    #[rustfmt::skip]
    let starts = [
        ('A', 0., 0.), ('B', 2., 2.), ('C', 6., 6.), ('D', 16., 20.), ('E', 16., 16.),
        ('F', 20., 20.), ('G', 22., 26.), ('H', 29., 33.), ('I', 16., 18.), ('J', 25., 25.),
        ('K', 33., 34.), ('L', 33., 33.), ('M', 38., 42.), ('N', 38., 38.),
    ];
    assert_cpm(&schedule, &ids, 44.0, "ABCEFJLN", &starts);
}

/// The mean critical path is not always the longest sampled path, so sampling must find
/// non-mean-critical activities critical in some schedules and no sampled finish below the
/// shortest possible one.
#[test]
fn reliable_construction_sampling_spreads_criticality_beyond_the_mean_path() {
    let (net, ids) = build(&RELIABLE);
    let config = SimulationConfig {
        iterations: 4_000,
        seed: 22,
    };
    let summary = simulate(&net.plan, config).expect("simulation");
    let criticality = |letter: char| summary.criticality[&ids[&letter]];
    for letter in ['A', 'B', 'C'] {
        assert!(
            (criticality(letter) - 1.0).abs() <= TOLERANCE,
            "{letter} lies on every path"
        );
    }
    for letter in ['D', 'G', 'H', 'M'] {
        assert!(criticality(letter) > 0.0, "{letter} is sometimes critical");
    }
    assert!(criticality('K') < criticality('L'));
    assert!(summary.p50_finish_hours > 40.0 && summary.p95_finish_hours > 47.0);
}

/// Cambridge International AS & A Level Computer Science 9608, topic 4.4.3 support guide,
/// section 2.2: expected times A=6, B=3, C=10, D=6, E=4, F=4, G=3, H=8 weeks. The expected
/// finish is asserted as computed from that published estimate table.
#[test]
fn cambridge_worked_example_matches_its_estimate_table() {
    let (net, ids) = build(&[
        ('A', "", 4., 5., 12.),
        ('B', "A", 2., 3., 4.),
        ('C', "B", 6., 8., 22.),
        ('D', "C", 4., 6., 8.),
        ('E', "C", 3., 4., 5.),
        ('F', "E", 2., 4., 6.),
        ('G', "DF", 2., 3., 4.),
        ('H', "C", 5., 7., 15.),
    ]);
    let schedule = deterministic(&net.plan).expect("schedule");
    #[rustfmt::skip]
    let starts = [
        ('A', 0., 0.), ('B', 6., 6.), ('C', 9., 9.), ('D', 19., 21.),
        ('E', 19., 19.), ('F', 23., 23.), ('G', 27., 27.), ('H', 19., 22.),
    ];
    assert_cpm(&schedule, &ids, 30.0, "ABCEFG", &starts);
}

/// The same guide, homework 2 and its solution: critical path A-B-D-F in 11 weeks, and one week
/// of slack for each of C and E.
#[test]
fn cambridge_homework_finds_the_published_critical_path() {
    let (net, ids) = build(&[
        ('A', "", 1., 1., 1.),
        ('B', "A", 3., 3., 3.),
        ('C', "A", 2., 2., 2.),
        ('D', "B", 3., 3., 3.),
        ('E', "C", 3., 3., 3.),
        ('F', "DE", 4., 4., 4.),
    ]);
    let schedule = deterministic(&net.plan).expect("schedule");
    #[rustfmt::skip]
    let starts = [('A', 0., 0.), ('B', 1., 1.), ('C', 1., 2.), ('D', 4., 4.), ('E', 3., 4.), ('F', 7., 7.)];
    assert_cpm(&schedule, &ids, 11.0, "ABDF", &starts);
}
