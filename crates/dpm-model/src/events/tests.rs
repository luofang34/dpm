use super::*;
use chrono::TimeZone;

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0)
        .single()
        .expect("fixed time")
}

fn hours(h: i64) -> DateTime<Utc> {
    t0() + TimeDelta::hours(h)
}

#[test]
fn positive_lag_releases_exactly_when_it_has_elapsed() {
    let event = Some(EventTime::Recorded(t0()));
    assert_eq!(
        Release::evaluate(event, 24.0, hours(23)),
        Release::Elapsing {
            event_at: t0(),
            opens_at: hours(24)
        }
    );
    assert_eq!(
        Release::evaluate(event, 24.0, hours(24)),
        Release::Released {
            at: EventTime::Recorded(hours(24))
        }
    );
    let almost = hours(24) - TimeDelta::milliseconds(1);
    assert!(matches!(
        Release::evaluate(event, 24.0, almost),
        Release::Elapsing { .. }
    ));
}

#[test]
fn negative_lag_never_releases_before_the_event() {
    assert_eq!(
        Release::evaluate(None, -48.0, hours(1000)),
        Release::AwaitingEvent
    );
    assert_eq!(
        Release::evaluate(Some(EventTime::Recorded(hours(5))), -48.0, hours(5)),
        Release::Released {
            at: EventTime::Recorded(hours(5))
        }
    );
    assert!(matches!(
        Release::evaluate(Some(EventTime::Recorded(hours(5))), -48.0, hours(4)),
        Release::Elapsing { .. }
    ));
}

#[test]
fn unrecorded_event_times_block_only_positive_lag() {
    let unknown = Some(EventTime::Unrecorded);
    assert_eq!(
        Release::evaluate(unknown, 0.0, t0()),
        Release::Released {
            at: EventTime::Unrecorded
        }
    );
    assert_eq!(
        Release::evaluate(unknown, -3.0, t0()),
        Release::Released {
            at: EventTime::Unrecorded
        }
    );
    assert_eq!(
        Release::evaluate(unknown, 0.5, hours(100_000)),
        Release::UnrecordedEventTime
    );
}

#[test]
fn unrepresentable_lag_stays_closed_without_panicking() {
    for lag in [1.0e300, f64::MAX] {
        assert_eq!(
            Release::evaluate(Some(EventTime::Recorded(t0())), lag, hours(1)),
            Release::LagOutOfRange { event_at: t0() }
        );
    }
}

#[test]
fn unknown_times_dominate_the_latest_event() {
    let known = EventTime::Recorded(t0());
    assert_eq!(
        known.latest(EventTime::Recorded(hours(2))),
        EventTime::Recorded(hours(2))
    );
    assert_eq!(known.latest(EventTime::Unrecorded), EventTime::Unrecorded);
    assert_eq!(EventTime::Unrecorded.latest(known), EventTime::Unrecorded);
}

#[test]
fn empty_events_are_omitted_and_old_snapshots_load() {
    let events = ExecutionEvents::default();
    assert!(events.is_empty());
    assert_eq!(serde_json::to_string(&events).expect("json"), "{}");
    let loaded: ExecutionEvents =
        serde_json::from_str(r#"{"started_at":"2026-09-01T12:00:00Z"}"#).expect("json");
    assert_eq!(loaded.started_at, Some(t0()));
}
