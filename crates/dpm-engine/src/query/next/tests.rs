use super::*;
use chrono::TimeZone;

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
}

fn execution() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

/// TEST-A is the only claimable task and heads the 76 h critical path; without its 4 h the
/// finish is 72 h, so its drag is 4 h whichever criticality basis ranks it.
#[test]
fn candidates_report_drag_and_name_their_criticality_basis_without_ranking_drag() {
    let plan = execution();
    for (probabilistic, basis, named) in [
        (
            true,
            CriticalityBasis::Simulated,
            "criticality basis is simulated",
        ),
        (
            false,
            CriticalityBasis::Deterministic,
            "criticality basis is deterministic",
        ),
    ] {
        let query = NextWorkQuery {
            capabilities: BTreeSet::new(),
            use_probabilistic_criticality: probabilistic,
        };
        let candidates = next_work(&plan, &query, at()).expect("next");
        let [first] = candidates.as_slice() else {
            panic!("only TEST-A is claimable: {candidates:?}");
        };
        assert_eq!(first.work.key.0, "TEST-A");
        assert_eq!(first.criticality_basis, basis);
        assert!((first.drag_hours - 4.0).abs() < 1e-9);
        assert!(
            first.reasons.iter().any(|r| r.starts_with(named)),
            "{:?}",
            first.reasons
        );
        assert!(
            first
                .reasons
                .iter()
                .any(|r| r.contains("drag 4.0h is reported but not ranked")),
            "{:?}",
            first.reasons
        );
        // No drag statement may read as the unestimated "0 h" statement.
        assert!(!first.reasons.iter().any(|r| r.contains("0 h")));
        let critical = first.criticality.unwrap_or(1.0);
        let score = critical * 100.0
            + 15.0
            + f64::from(first.work.schedule.priority.rank()) * 8.0
            + libm::log(first.downstream_count as f64 + 1.0) * 6.0
            - first.total_float_hours.min(100.0) * 0.05;
        assert!(
            (first.score - score).abs() < 1e-9,
            "drag stays out of the score"
        );
    }
}
