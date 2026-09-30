use super::*;
use crate::query::calibration::tests::Log;

fn close(actual: Option<f64>, expected: f64) {
    let actual = actual.expect("measured");
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn cycle_and_lead_time_run_from_start_and_first_claim_to_verification() {
    let flow = Log::scenario().report(50.0).flow;
    // Start to verification: C 1 h, E 2 h, A 4 h, B 13 h 50 min, D 30 h.
    let b = 20.0 - (6.0 + 10.0 / 60.0);
    assert_eq!(flow.cycle_time.count, 5);
    close(flow.cycle_time.median_hours, 4.0);
    close(flow.cycle_time.p85_hours, b + 0.4 * (30.0 - b));
    // First claim to verification: C 1.5 h, E 3 h, A 5 h, B 14 h, D 33 h 40 min.
    assert_eq!(flow.lead_time.count, 5);
    close(flow.lead_time.median_hours, 5.0);
}

#[test]
fn throughput_counts_verifications_by_period_and_iso_week() {
    let flow = Log::scenario().report(50.0).flow;
    assert_eq!(
        (flow.throughput.last_7_days, flow.throughput.last_28_days),
        (5, 5)
    );
    let weeks: Vec<_> = flow
        .throughput
        .weeks
        .iter()
        .map(|w| (w.week.as_str(), w.verified))
        .collect();
    assert_eq!(weeks.len(), THROUGHPUT_WEEKS);
    assert_eq!(weeks.first(), Some(&("2026-W29", 0)));
    assert_eq!(weeks.last(), Some(&("2026-W36", 5)));

    let later = Log::scenario().report(50.0 + 24.0 * 10.0).flow;
    assert_eq!(
        (later.throughput.last_7_days, later.throughput.last_28_days),
        (0, 5)
    );
}

#[test]
fn aging_lists_held_work_from_the_current_holders_claim() {
    let flow = Log::scenario().report(50.0).flow;
    let aging: Vec<_> = flow
        .aging
        .iter()
        .map(|w| {
            (
                w.key.0.as_str(),
                w.status,
                w.owner.as_ref().map(ToString::to_string),
            )
        })
        .collect();
    assert_eq!(
        aging,
        [(
            "TEST-F",
            WorkStatus::Claimed,
            Some("agent:third".to_string())
        )]
    );
    let held = flow.aging.first().expect("held");
    close(held.hours_since_claim, 2.0);
    assert_eq!(held.hours_since_start, None);
}

#[test]
fn claim_episodes_end_verified_released_handed_off_or_open() {
    let reliability = Log::scenario().report(50.0).flow.reliability;
    let total = reliability.total;
    assert_eq!(
        (
            total.episodes,
            total.verified,
            total.verified_after_rejection,
            total.released,
            total.handed_off,
            total.open
        ),
        (8, 5, 1, 1, 1, 1)
    );
    close(total.verified_fraction, 5.0 / 7.0);
    let holders: Vec<_> = reliability
        .by_holder
        .iter()
        .map(|h| (h.holder, h.outcomes.episodes, h.outcomes.verified))
        .collect();
    assert_eq!(
        holders,
        [(ActorKind::Human, 2, 2), (ActorKind::Agent, 6, 3)]
    );
    let json = serde_json::to_value(&reliability.by_holder).expect("json");
    assert_eq!(json[0]["holder"], "Human");
    assert_eq!(json[0]["verified"], 2);
}

#[test]
fn a_plan_without_history_reports_empty_flow() {
    let flow = Log::new().report(0.0).flow;
    assert_eq!(flow.cycle_time.count, 0);
    assert_eq!(flow.cycle_time.median_hours, None);
    assert_eq!(flow.reliability.total.verified_fraction, None);
    assert!(flow.aging.is_empty());
}
