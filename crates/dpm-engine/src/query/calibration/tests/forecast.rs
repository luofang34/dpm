//! What a calibrated forecast refuses to apply, and how it says so.

use super::*;

/// The scenario's calibration with every executor group sufficient at `median`.
fn with_median(median: f64) -> (Log, CalibrationReport) {
    let log = Log::scenario();
    let mut report = log.report(50.0);
    for group in &mut report.estimates.by_executor {
        group.sufficient = true;
        group.median = Some(median);
    }
    (log, report)
}

#[test]
fn a_median_outside_the_band_is_reported_but_never_applied() {
    for median in [0.0, 0.001, 60.0, f64::NAN] {
        let (log, report) = with_median(median);
        let plain = status(&log.plan, false, at(50.0)).expect("status");
        let calibrated = status_calibrated(&log.plan, false, at(50.0), &report).expect("status");
        close(
            calibrated.expected_finish_hours,
            plain.expected_finish_hours,
        );
        let applied = calibrated.calibration.expect("calibration");
        let agent = applied.factors.first().expect("factor");
        assert!(!agent.applied, "{median}");
        close(agent.factor, 1.0);
        assert_eq!(agent.measured.map(f64::to_bits), Some(median.to_bits()));
        assert!(agent.reason.contains("outside"), "{}", agent.reason);
    }
}

#[test]
fn the_band_edges_are_applied_per_executor_kind() {
    for median in [
        MIN_AGENT_APPLIED_RATIO,
        MIN_APPLIED_RATIO,
        MAX_APPLIED_RATIO,
    ] {
        let (log, report) = with_median(median);
        let calibrated = status_calibrated(&log.plan, false, at(50.0), &report).expect("status");
        for factor in calibrated.calibration.expect("calibration").factors {
            let floor = min_applied_ratio(factor.executor);
            assert_eq!(
                factor.applied,
                median >= floor,
                "{median} {:?}",
                factor.executor
            );
            if !factor.applied {
                assert!(factor.reason.contains("outside"), "{}", factor.reason);
            }
        }
    }
    assert_eq!(min_applied_ratio(ActorKind::Human), MIN_APPLIED_RATIO);
    assert_eq!(min_applied_ratio(ActorKind::Service), MIN_APPLIED_RATIO);
    assert_eq!(min_applied_ratio(ActorKind::Agent), MIN_AGENT_APPLIED_RATIO);
    let report = Log::scenario().report(50.0);
    assert_eq!(
        (
            report.rules.min_applied_ratio,
            report.rules.min_agent_applied_ratio,
            report.rules.max_applied_ratio
        ),
        (
            MIN_APPLIED_RATIO,
            MIN_AGENT_APPLIED_RATIO,
            MAX_APPLIED_RATIO
        )
    );
}

#[test]
fn reasons_name_actor_kinds_in_plain_words() {
    let log = Log::scenario();
    let report = log.report(50.0);
    let calibrated = status_calibrated(&log.plan, false, at(50.0), &report).expect("status");
    let applied = calibrated.calibration.expect("calibration");
    let agent = applied.factors.first().expect("factor");
    assert_eq!(
        agent.reason,
        "2 verified agent sample(s), fewer than the 5 needed; estimates kept"
    );
    assert_eq!(
        applied.review_delay.reason,
        "3 human review(s) measured, fewer than the 5 needed; no review delay added"
    );
}
