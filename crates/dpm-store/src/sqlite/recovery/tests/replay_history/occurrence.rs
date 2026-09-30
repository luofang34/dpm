//! A log recording lifecycle occurrence times replays to its snapshot, keeping both times.

use super::*;

#[test]
fn backfilled_lifecycle_events_replay_with_their_occurrence_times() {
    let dir = tempfile::tempdir().expect("directory");
    let mut r = Recorder::new(dir.path());
    let a = r.id("TEST-A");
    let gate = r.plan().find_decision_by_key("TEST-GATE").expect("gate").id;
    let outcome = text("proceed");
    r.run(
        lead(),
        Command::Decide {
            decision: gate,
            outcome,
        },
    );
    r.run(author(), Command::Claim { work: a });
    // Each event occurred an hour before the operation that records it commits.
    let occurred_at = Some(t(r.hour));
    r.run(
        author(),
        Command::Start {
            work: a,
            occurred_at,
        },
    );
    let occurred_at = Some(t(r.hour));
    let note = None;
    r.run(
        author(),
        Command::Submit {
            work: a,
            note,
            occurred_at,
        },
    );
    let occurred_at = Some(t(r.hour));
    let note = None;
    r.run(
        reviewer(),
        Command::Verify {
            work: a,
            note,
            occurred_at,
        },
    );
    let events = r.plan().work_items[&a].execution.events;
    assert_eq!(
        (events.started_at, events.submitted_at, events.verified_at),
        (Some(t(2)), Some(t(3)), Some(t(4)))
    );
    let history = r.store.history_blocking(0, 10).expect("history");
    let verify = history
        .entries
        .last()
        .expect("verify")
        .operation
        .operation
        .clone();
    assert_eq!(verify.timestamp, t(5));
    let Command::Verify { occurred_at, .. } = verify.command else {
        panic!("expected the verify operation, got {:?}", verify.command);
    };
    assert_eq!(occurred_at, Some(t(4)));
}
