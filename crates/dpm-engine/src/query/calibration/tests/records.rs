//! Records that cannot say how long work took, and rework whose review wait is measured apart.

use super::*;

fn reasons(report: &CalibrationReport) -> Vec<(ExclusionReason, Vec<&str>)> {
    report
        .estimates
        .excluded
        .iter()
        .map(|e| (e.reason, keys(e)))
        .collect()
}

#[test]
fn a_start_and_submit_backfilled_to_one_instant_is_no_working_time() {
    let mut log = Log::new();
    log.claim("agent:coder", "TEST-A", 0.0);
    log.start("agent:coder", "TEST-A", 1.0, Some(0.5));
    log.submit("agent:coder", "TEST-A", 2.0, Some(0.5));
    log.verify("human:bob", "TEST-A", 3.0);
    let report = log.report(4.0);
    assert!(report.estimates.samples.is_empty());
    assert_eq!(
        reasons(&report),
        [(ExclusionReason::NoWorkingTime, vec!["TEST-A"])]
    );
    let json = serde_json::to_value(&report.estimates.excluded).expect("json");
    assert_eq!(json[0]["reason"], "no_working_time");
    assert_eq!(report.rules.min_actual_seconds, MIN_ACTUAL_SECONDS);
}

#[test]
fn work_recorded_entirely_outside_working_hours_is_no_working_time() {
    let mut log = Log::new();
    log.plan.calendars = Some(Calendars::in_zone("Europe/Berlin"));
    // Saturday 5 September 2026, 10:00 to 14:00 in Berlin: four elapsed hours, none working.
    let saturday = 4.0 * 24.0 + 8.0;
    log.claim("human:alice", "TEST-A", saturday - 1.0);
    log.start("human:alice", "TEST-A", saturday, None);
    log.submit("human:alice", "TEST-A", saturday + 4.0, None);
    log.verify("human:bob", "TEST-A", saturday + 5.0);
    let report = log.report(saturday + 6.0);
    assert_eq!(
        reasons(&report),
        [(ExclusionReason::NoWorkingTime, vec!["TEST-A"])]
    );
}

#[test]
fn a_minute_of_work_is_the_smallest_measurement() {
    for (minutes, measured) in [(1.0, true), (0.5, false)] {
        let mut log = Log::new();
        log.claim("agent:coder", "TEST-A", 0.0);
        log.start("agent:coder", "TEST-A", 1.0, None);
        log.submit("agent:coder", "TEST-A", 1.0 + minutes / 60.0, None);
        log.verify("human:bob", "TEST-A", 2.0);
        let report = log.report(3.0);
        assert_eq!(
            report.estimates.samples.len(),
            usize::from(measured),
            "{minutes} min"
        );
    }
}

#[test]
fn a_calendar_that_cannot_count_the_interval_excludes_only_that_task() {
    let mut log = Log::new();
    log.plan.calendars = Some(Calendars::in_zone("Europe/Berlin"));
    // Started more than a century before its submission: longer than a calendar window spans.
    let century = -24.0 * 366.0 * 101.0;
    log.claim("human:alice", "TEST-A", century);
    log.start("human:alice", "TEST-A", century, None);
    log.submit("human:alice", "TEST-A", 10.0, None);
    log.verify("human:bob", "TEST-A", 11.0);
    let report = log.report(12.0);
    assert_eq!(
        reasons(&report),
        [(ExclusionReason::CalendarOutOfRange, vec!["TEST-A"])]
    );
    assert!(status_calibrated(&log.plan, false, at(12.0), &report).is_ok());
}

#[test]
fn review_waits_of_rejected_attempts_are_not_execution_time() {
    let mut log = Log::new();
    log.claim("agent:coder", "TEST-A", 0.0);
    log.start("agent:coder", "TEST-A", 0.0, None);
    log.submit("agent:coder", "TEST-A", 2.0, None);
    log.reject("human:bob", "TEST-A", 12.0);
    log.submit("agent:coder", "TEST-A", 13.0, None);
    log.verify("human:bob", "TEST-A", 14.0);
    let report = log.report(15.0);
    // 13 h from start to the verified submission, of which the rejected attempt waited 10 h for
    // its review; that wait is a review sample, not work.
    let sample = report.estimates.samples.first().expect("sample");
    close(sample.actual_hours, 3.0);
    let reviews: Vec<_> = report.reviews.by_kind.iter().map(|g| g.count).collect();
    assert_eq!(reviews, [2]);
}

#[test]
fn an_attempt_handed_between_kinds_is_not_attributed_to_either() {
    let mut log = Log::new();
    let work = log.id("TEST-A");
    log.claim("human:alice", "TEST-A", 0.0);
    log.start("human:alice", "TEST-A", 1.0, None);
    let handoff = Command::Handoff {
        work,
        from: actor("human:alice"),
        to: actor("agent:coder"),
        reason: "continue overnight".into(),
    };
    log.run("human:lead", handoff, 2.0);
    log.submit("agent:coder", "TEST-A", 5.0, None);
    log.verify("human:bob", "TEST-A", 6.0);
    let report = log.report(7.0);
    assert_eq!(
        reasons(&report),
        [(ExclusionReason::MixedExecutors, vec!["TEST-A"])]
    );
    // Counted against the kind that submitted, beside that kind's (empty) distribution.
    let agent = report.estimates.by_executor.first().expect("group");
    assert_eq!(
        (agent.executor, agent.samples, agent.median),
        (ActorKind::Agent, 0, None)
    );
    let excluded: Vec<_> = agent.excluded.iter().map(|r| (r.reason, r.count)).collect();
    assert_eq!(excluded, [(ExclusionReason::MixedExecutors, 1)]);
}

#[test]
fn a_handoff_within_one_kind_keeps_the_sample() {
    let mut log = Log::new();
    let work = log.id("TEST-A");
    log.claim("agent:first", "TEST-A", 0.0);
    log.start("agent:first", "TEST-A", 1.0, None);
    let handoff = Command::Handoff {
        work,
        from: actor("agent:first"),
        to: actor("agent:second"),
        reason: "rebalance".into(),
    };
    log.run("human:lead", handoff, 2.0);
    log.submit("agent:second", "TEST-A", 3.0, None);
    log.verify("human:bob", "TEST-A", 4.0);
    let report = log.report(5.0);
    assert_eq!(report.estimates.samples.len(), 1);
}
