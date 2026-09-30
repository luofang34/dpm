//! Which records count as typed in after the fact, and how visible their exclusion is.

use super::*;

/// TEST-A claimed at `claimed`, then started at 5 h and submitted `seconds` later, neither
/// backfilled.
fn quick(claimed: f64, seconds: f64) -> Log {
    let mut log = Log::new();
    log.claim("agent:coder", "TEST-A", claimed);
    log.start("agent:coder", "TEST-A", 5.0, None);
    log.submit("agent:coder", "TEST-A", 5.0 + seconds / 3600.0, None);
    log.verify("human:bob", "TEST-A", 6.0);
    log
}

#[test]
fn quick_work_claimed_well_before_is_measured() {
    // Held for five hours before a two-minute start-to-submit: genuinely short work.
    let report = quick(0.0, 120.0).report(7.0);
    let sample = report.estimates.samples.first().expect("sample");
    close(sample.actual_hours, 2.0 / 60.0);
    assert!(report.estimates.excluded.is_empty());
    assert_eq!(report.flow.cycle_time.count, 1);
}

#[test]
fn claim_start_and_submit_in_one_sitting_are_bulk_recorded() {
    let report = quick(5.0 - 60.0 / 3600.0, 120.0).report(7.0);
    let bulk = report.estimates.excluded.first().expect("excluded");
    assert_eq!(
        (bulk.reason, keys(bulk)),
        (ExclusionReason::BulkRecorded, vec!["TEST-A"])
    );
    assert_eq!(report.flow.cycle_time.count, 0);
}

#[test]
fn without_the_claim_in_history_start_and_submit_alone_decide() {
    let mut log = quick(0.0, 120.0);
    // The claim predates the log the store holds.
    log.history.remove(0);
    let report = log.report(7.0);
    let bulk = report.estimates.excluded.first().expect("excluded");
    assert_eq!(bulk.reason, ExclusionReason::BulkRecorded);
}

#[test]
fn a_backfilled_start_is_never_bulk_recorded() {
    let mut log = Log::new();
    log.claim("agent:coder", "TEST-A", 5.0);
    log.start("agent:coder", "TEST-A", 5.0, Some(5.0));
    log.submit(
        "agent:coder",
        "TEST-A",
        5.0 + 60.0 / 3600.0,
        Some(5.0 + 30.0 / 3600.0),
    );
    log.verify("human:bob", "TEST-A", 6.0);
    let report = log.report(7.0);
    // Honest times, only too short to measure.
    let excluded = report.estimates.excluded.first().expect("excluded");
    assert_eq!(excluded.reason, ExclusionReason::NoWorkingTime);
}

#[test]
fn each_executor_group_counts_what_it_left_out() {
    let report = Log::scenario().report(50.0);
    let groups: Vec<_> = report
        .estimates
        .by_executor
        .iter()
        .map(|g| {
            let excluded: Vec<_> = g.excluded.iter().map(|r| (r.reason, r.count)).collect();
            (g.executor, g.samples, excluded)
        })
        .collect();
    assert_eq!(
        groups,
        [
            (ActorKind::Human, 1, vec![(ExclusionReason::Unestimated, 1)]),
            (
                ActorKind::Agent,
                2,
                vec![(ExclusionReason::BulkRecorded, 1)]
            ),
        ]
    );
    let json = serde_json::to_value(&report.estimates.by_executor).expect("json");
    assert_eq!(
        json[1]["excluded"],
        serde_json::json!([{"reason": "bulk_recorded", "count": 1}])
    );
}
