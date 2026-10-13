//! A project recorded entirely after the fact: its decision, claims and lifecycle events carry the
//! times they occurred, entered in that order long after they happened.

use super::*;
use dpm_model::{Decision, DecisionId, DecisionStatus, Key};
use std::collections::BTreeSet;

fn lead() -> ActorId {
    ActorId::human("lead")
}

/// The pair with an open decision gating its first task.
fn gated() -> (Plan, WorkItemId, WorkItemId, DecisionId) {
    let (mut plan, a, b) = pair(DependencyKind::FinishStart, 0.0, DependencyPolicy::Hard);
    let decision = Decision {
        id: DecisionId::new(),
        key: Key::new("GO"),
        project: plan.work_items[&a].project,
        question: "Proceed?".into(),
        status: DecisionStatus::Open,
        outcome: None,
        resolved_at: None,
        rationale: None,
        related_work: BTreeSet::new(),
        artifact_ids: BTreeSet::new(),
        blocks: BTreeSet::from([a]),
        supersedes: None,
        options: Vec::new(),
    };
    let id = decision.id;
    plan.decisions.insert(id, decision);
    plan.validate().expect("gated pair");
    (plan, a, b, id)
}

fn decide(decision: DecisionId, hour: i64) -> Command {
    Command::Decide {
        decision,
        outcome: "Proceed".into(),
        occurred_at: Some(t(hour)),
    }
}

fn claim(work: WorkItemId, hour: i64) -> Command {
    Command::Claim {
        work,
        occurred_at: Some(t(hour)),
    }
}

fn at(hour: i64) -> Option<DateTime<Utc>> {
    Some(t(hour))
}

#[test]
fn a_lifecycle_entered_in_event_order_keeps_every_occurrence_time() {
    let (mut plan, a, b, gate) = gated();
    ok(&mut plan, &lead(), decide(gate, 1), 100);
    assert_eq!(plan.decisions[&gate].resolved_at, at(1));
    ok(&mut plan, &worker(), claim(a, 2), 101);
    assert_eq!(plan.work_items[&a].execution.events.claimed_at, at(2));
    for (actor, command) in [
        (
            worker(),
            Command::Start {
                work: a,
                occurred_at: at(3),
            },
        ),
        (
            worker(),
            Command::Submit {
                work: a,
                note: None,
                occurred_at: at(4),
            },
        ),
        (
            lead(),
            Command::Verify {
                work: a,
                note: None,
                occurred_at: at(5),
            },
        ),
    ] {
        ok(&mut plan, &actor, command, 102);
    }
    ok(&mut plan, &worker(), claim(b, 6), 103);
    let events = &plan.work_items[&a].execution.events;
    assert_eq!((events.started_at, events.verified_at), (at(3), at(5)));
    assert_eq!(plan.work_items[&b].execution.events.claimed_at, at(6));
}

#[test]
fn a_backdated_claim_meets_the_gates_and_history_of_its_own_time() {
    let (mut plan, a, b, gate) = gated();
    ok(&mut plan, &lead(), decide(gate, 5), 100);
    let release = refused(&mut plan, &worker(), claim(b, 4), 100);
    assert_eq!(
        release,
        Release::AwaitingEvent,
        "its predecessor had not finished"
    );
    let before = plan.clone();
    let error = run(&mut plan, &worker(), claim(a, 4), 100).expect_err("gate was open at 4");
    assert!(
        matches!(&error, EngineError::NotReady { unmet, .. }
            if unmet.iter().any(|g| matches!(g, UnmetGate::Decision { .. }))),
        "{error:?}"
    );
    assert_eq!(plan, before);
    let error = run(&mut plan, &worker(), claim(a, 101), 100).expect_err("future");
    assert!(
        matches!(error, EngineError::OccurrenceInFuture { .. }),
        "{error:?}"
    );
    ok(&mut plan, &worker(), claim(a, 6), 100);
    ok(
        &mut plan,
        &worker(),
        Command::Release {
            work: a,
            reason: "handing back".into(),
        },
        100,
    );
    let error = run(&mut plan, &worker(), claim(a, 7), 101).expect_err("before the release");
    assert!(
        matches!(error, EngineError::OccurrenceBeforePrevious { .. }),
        "{error:?}"
    );
}

#[test]
fn a_decision_cannot_be_recorded_after_its_commit() {
    let (mut plan, _, _, gate) = gated();
    let before = plan.clone();
    let error = run(&mut plan, &lead(), decide(gate, 11), 10).expect_err("future");
    assert!(
        matches!(&error, EngineError::InvalidCommand { reason, .. } if reason.contains("after the operation commits")),
        "{error:?}"
    );
    assert_eq!(plan, before);
    ok(&mut plan, &lead(), decide(gate, 10), 10);
}
