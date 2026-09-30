use crate::*;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

#[test]
fn an_unrecorded_claim_contributes_no_time() {
    let record = ExecutionRecord {
        status: WorkStatus::Claimed,
        owner: Some(ActorId::agent("worker")),
        ..ExecutionRecord::default()
    };
    assert_eq!(record.latest_recorded_at(), None);
}

#[test]
fn every_timed_record_can_be_the_latest() {
    let review = |at| AttemptOutcome::Rejected {
        actor: ActorId::human("reviewer"),
        at,
        reason: "missing test".into(),
    };
    let mut record = ExecutionRecord::default();
    record.events.started_at = Some(t(1));
    assert_eq!(record.latest_recorded_at(), Some(t(1)));
    record.releases.push(ClaimRelease {
        actor: ActorId::agent("worker"),
        at: t(2),
        reason: "mistaken".into(),
    });
    assert_eq!(record.latest_recorded_at(), Some(t(2)));
    record.attempts.push(SubmissionAttempt {
        number: 1,
        submitted_at: t(3),
        outcome: review(t(4)),
    });
    assert_eq!(record.latest_recorded_at(), Some(t(4)));
    record.handoffs.push(Handoff {
        from: ActorId::agent("worker"),
        to: ActorId::agent("finisher"),
        actor: ActorId::human("lead"),
        at: t(5),
        reason: "reassigned".into(),
    });
    assert_eq!(record.latest_recorded_at(), Some(t(5)));
    record.basis.push(DependencyBasis {
        dependency: DependencyId::new(),
        predecessor: WorkItemId::new(),
        attempt: 1,
        recorded_at: t(6),
        source: BasisSource::Start,
    });
    assert_eq!(record.latest_recorded_at(), Some(t(6)));
}
