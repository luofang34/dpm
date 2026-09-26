//! Seeded property tests comparing the projection with an independent relaxation oracle.

use super::{KINDS, Network, TOLERANCE, stable_id};
use dpm_model::{DependencyKind, WorkItemId};
use dpm_schedule::{Schedule, ScheduleError, deterministic_with_durations};
use std::collections::BTreeMap;

const SEED: u64 = 0x5eed_0010;
const CASES: u64 = 300;
/// Probe delay: well above the tolerance and below any generated slack granularity.
const DELTA: f64 = 1e-3;

/// SplitMix64: tiny, well-distributed and fully reproducible from the seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// Mostly binary-exact quarter hours, sometimes thirds to exercise rounding.
    fn hours(&mut self, range: i64) -> f64 {
        let span = u64::try_from(2 * range + 1).unwrap_or(1);
        let step = i64::try_from(self.below(span)).unwrap_or(0) - range;
        let unit = if self.below(4) == 0 { 3.0 } else { 4.0 };
        step as f64 / unit
    }
}

struct Edge {
    from: usize,
    to: usize,
    kind: DependencyKind,
    lag: f64,
}

struct Case {
    ids: Vec<WorkItemId>,
    durations: Vec<f64>,
    edges: Vec<Edge>,
}

impl Case {
    /// Edges only point from lower to higher generation index, so the graph is acyclic; identities
    /// are shuffled so identity order does not coincide with topological order.
    fn generate(rng: &mut Rng) -> Self {
        let n = 2 + usize::try_from(rng.below(11)).unwrap_or(0);
        let mut ids: Vec<_> = (1..=n as u64).map(stable_id).collect();
        for i in (1..n).rev() {
            let j = usize::try_from(rng.below(i as u64 + 1)).unwrap_or(0);
            ids.swap(i, j);
        }
        let durations = (0..n)
            .map(|_| match rng.below(5) {
                0 => 0.0,
                _ => rng.hours(40).abs(),
            })
            .collect();
        let mut edges = Vec::new();
        for to in 1..n {
            for from in 0..to {
                if rng.below(10) < 3 {
                    edges.push(Edge {
                        from,
                        to,
                        kind: KINDS[usize::try_from(rng.below(4)).unwrap_or(0)],
                        lag: rng.hours(24),
                    });
                }
            }
        }
        Self {
            ids,
            durations,
            edges,
        }
    }

    fn network(&self) -> Network {
        let mut net = Network::new();
        for (id, hours) in self.ids.iter().zip(&self.durations) {
            if *hours == 0.0 && id.0.as_u128() % 2 == 0 {
                net.milestone(*id);
            } else {
                net.task(*id, *hours);
            }
        }
        for e in &self.edges {
            net.link(self.ids[e.from], self.ids[e.to], e.kind, e.lag);
        }
        net
    }

    /// Offset between the two constrained endpoints, derived from which endpoint each side names.
    fn weight(&self, e: &Edge) -> f64 {
        let (from_finish, to_finish) = match e.kind {
            DependencyKind::FinishStart => (true, false),
            DependencyKind::StartStart => (false, false),
            DependencyKind::FinishFinish => (true, true),
            DependencyKind::StartFinish => (false, true),
        };
        let from = if from_finish {
            self.durations[e.from]
        } else {
            0.0
        };
        let to = if to_finish { self.durations[e.to] } else { 0.0 };
        from + e.lag - to
    }

    /// Bellman-Ford style earliest starts with optional release times; no topological order used.
    fn earliest(&self, release: &[f64]) -> (Vec<f64>, f64) {
        let mut es = release.to_vec();
        for _ in 0..=self.ids.len() {
            for e in &self.edges {
                es[e.to] = es[e.to].max(es[e.from] + self.weight(e));
            }
        }
        let finish = es
            .iter()
            .zip(&self.durations)
            .fold(0.0_f64, |m, (s, d)| m.max(s + d));
        (es, finish)
    }

    fn latest(&self, finish: f64) -> Vec<f64> {
        let mut ls: Vec<_> = self.durations.iter().map(|d| finish - d).collect();
        for _ in 0..=self.ids.len() {
            for e in &self.edges {
                ls[e.from] = ls[e.from].min(ls[e.to] - self.weight(e));
            }
        }
        ls
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOLERANCE
}

fn check_bounds(case: &Case, schedule: &Schedule, label: &str) {
    let zero = vec![0.0; case.ids.len()];
    let (es, finish) = case.earliest(&zero);
    let ls = case.latest(finish);
    assert!(
        close(schedule.project_finish_hours, finish),
        "{label}: finish"
    );
    for (i, id) in case.ids.iter().enumerate() {
        let a = &schedule.activities[id];
        let d = case.durations[i];
        assert!(a.earliest_start_hours >= 0.0, "{label}: ES before origin");
        assert!(close(a.earliest_start_hours, es[i]), "{label}: ES {i}");
        assert!(close(a.latest_start_hours, ls[i]), "{label}: LS {i}");
        assert!(close(a.earliest_finish_hours, a.earliest_start_hours + d));
        assert!(close(a.latest_finish_hours, a.latest_start_hours + d));
        assert!(
            a.latest_finish_hours <= finish + TOLERANCE,
            "{label}: LF {i}"
        );
        assert!(
            a.latest_start_hours >= a.earliest_start_hours - TOLERANCE,
            "{label}: raw LS < ES for {i}"
        );
        assert!(close(a.total_float_hours, (ls[i] - es[i]).max(0.0)));
        assert!(a.free_float_hours >= 0.0, "{label}: negative free float");
        assert!(
            a.free_float_hours <= a.total_float_hours,
            "{label}: free float exceeds total float for {i}"
        );
        assert_eq!(a.critical, a.total_float_hours <= TOLERANCE, "{label}");
        assert_eq!(schedule.critical_activities.contains(id), a.critical);
        if close(a.earliest_finish_hours, finish) {
            assert!(a.critical, "{label}: finishing activity {i} not critical");
        }
    }
    for e in &case.edges {
        let (from, to) = (
            &schedule.activities[&case.ids[e.from]],
            &schedule.activities[&case.ids[e.to]],
        );
        let w = case.weight(e);
        assert!(to.earliest_start_hours + TOLERANCE >= from.earliest_start_hours + w);
        assert!(to.latest_start_hours + TOLERANCE >= from.latest_start_hours + w);
    }
}

/// Delaying an activity by its float must be absorbed; delaying it slightly further must not.
fn check_float_semantics(case: &Case, schedule: &Schedule, label: &str) {
    let zero = vec![0.0; case.ids.len()];
    let (es, finish) = case.earliest(&zero);
    for (i, id) in case.ids.iter().enumerate() {
        let a = &schedule.activities[id];
        let delayed = |by: f64| {
            let mut release = zero.clone();
            release[i] = es[i] + by;
            case.earliest(&release)
        };
        let (_, absorbed) = delayed(a.total_float_hours);
        assert!(
            close(absorbed, finish),
            "{label}: total float of {i} moves finish"
        );
        let (_, pushed) = delayed(a.total_float_hours + DELTA);
        assert!(
            pushed >= finish + DELTA - TOLERANCE,
            "{label}: total float of {i} not tight"
        );

        let (kept, kept_finish) = delayed(a.free_float_hours);
        let others_unchanged = (0..es.len()).all(|j| j == i || close(kept[j], es[j]));
        assert!(
            others_unchanged && close(kept_finish, finish),
            "{label}: free float {i}"
        );
        let (moved, moved_finish) = delayed(a.free_float_hours + DELTA);
        let disturbed = (0..es.len()).any(|j| j != i && moved[j] > es[j] + DELTA / 2.0)
            || moved_finish > finish + DELTA / 2.0;
        assert!(disturbed, "{label}: free float of {i} not tight");
    }
}

#[test]
fn random_networks_match_independent_oracle_and_float_invariants() {
    let mut rng = Rng(SEED);
    let mut relations = BTreeMap::new();
    for case_index in 0..CASES {
        let case = Case::generate(&mut rng);
        for e in &case.edges {
            let seen = relations
                .entry(format!("{:?}/{}", e.kind, e.lag < 0.0))
                .or_insert(0_u32);
            *seen = seen.wrapping_add(1);
        }
        let net = case.network();
        let label = format!("seed {SEED:#x} case {case_index}");
        let before = net.plan.clone();
        let schedule = deterministic_with_durations(&net.plan, net.durations()).expect(&label);
        check_bounds(&case, &schedule, &label);
        check_float_semantics(&case, &schedule, &label);
        assert!(
            !schedule.critical_activities.is_empty(),
            "{label}: no critical path"
        );

        let again = deterministic_with_durations(&net.plan, net.durations()).expect(&label);
        let mut reordered = net.plan.clone();
        reordered.dependencies.reverse();
        let permuted = deterministic_with_durations(&reordered, net.durations()).expect(&label);
        let encoded = serde_json::to_string(&schedule).expect("encode");
        assert_eq!(
            encoded,
            serde_json::to_string(&again).expect("encode"),
            "{label}"
        );
        assert_eq!(
            encoded,
            serde_json::to_string(&permuted).expect("encode"),
            "{label}"
        );
        assert_eq!(
            net.plan, before,
            "{label}: projection must not edit the plan"
        );
    }
    // Every relation must be exercised with both lag and lead.
    assert_eq!(relations.len(), 8, "{relations:?}");
}

#[test]
fn back_edges_on_random_networks_are_rejected_as_cycles() {
    let mut rng = Rng(SEED ^ 0xc1c1e);
    let mut rejected = 0_u32;
    for case_index in 0..CASES {
        let case = Case::generate(&mut rng);
        let Some(edge) = case
            .edges
            .get(usize::try_from(rng.below(case.edges.len().max(1) as u64)).unwrap_or(0))
        else {
            continue;
        };
        let mut net = case.network();
        let kind = KINDS[usize::try_from(rng.below(4)).unwrap_or(0)];
        net.link(case.ids[edge.to], case.ids[edge.from], kind, rng.hours(24));
        let before = net.plan.clone();
        assert!(
            matches!(
                deterministic_with_durations(&net.plan, net.durations()),
                Err(ScheduleError::DependencyCycle)
            ),
            "seed case {case_index}"
        );
        assert_eq!(net.plan, before);
        rejected = rejected.wrapping_add(1);
    }
    assert!(
        rejected > CASES as u32 / 2,
        "too few cyclic cases: {rejected}"
    );
}
