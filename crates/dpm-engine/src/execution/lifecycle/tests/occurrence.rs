//! Lifecycle events recorded after the fact keep their occurrence time apart from the commit time.

use super::*;
use crate::Operation;
use dpm_model::StartBasis;

fn reviewer() -> ActorId {
    ActorId::human("reviewer")
}

fn start_at(work: WorkItemId, hour: i64) -> Command {
    Command::Start {
        work,
        occurred_at: Some(t(hour)),
    }
}

fn submit_at(work: WorkItemId, hour: i64) -> Command {
    Command::Submit {
        work,
        note: None,
        occurred_at: Some(t(hour)),
    }
}

fn verify_at(work: WorkItemId, hour: i64) -> Command {
    Command::Verify {
        work,
        note: None,
        occurred_at: Some(t(hour)),
    }
}

fn commit(plan: &mut Plan, actor: &ActorId, command: Command, hour: i64) -> Operation {
    let label = format!("{command:?} committed at +{hour}h");
    apply_command(
        plan,
        actor.clone(),
        command,
        t(hour),
        dpm_model::OperationId::new(),
    )
    .unwrap_or_else(|e| panic!("{label}: {e}"))
}

/// A refused occurrence leaves the plan, revision included, unchanged.
fn refused_at(plan: &mut Plan, actor: &ActorId, command: Command, hour: i64) -> EngineError {
    let before = plan.clone();
    let error = run(plan, actor, command, hour).expect_err("occurrence must be refused");
    assert_eq!(*plan, before, "a rejected command changes nothing");
    error
}

#[test]
fn backfilled_events_are_recorded_while_operations_keep_their_commit_time() {
    let (mut plan, a, _) = pair(DependencyKind::FinishStart, 0.0, DependencyPolicy::Hard);
    ok(&mut plan, &worker(), Command::Claim { work: a }, 0);
    let started = commit(&mut plan, &worker(), start_at(a, 2), 10);
    let submitted = commit(&mut plan, &worker(), submit_at(a, 4), 11);
    let verified = commit(&mut plan, &reviewer(), verify_at(a, 6), 12);
    let times: Vec<_> = [&started, &submitted, &verified]
        .iter()
        .map(|op| op.timestamp)
        .collect();
    assert_eq!(times, [t(10), t(11), t(12)]);
    let execution = &plan.work_items[&a].execution;
    assert_eq!(
        (
            execution.events.started_at,
            execution.events.submitted_at,
            execution.events.verified_at
        ),
        (Some(t(2)), Some(t(4)), Some(t(6)))
    );
    let attempt = execution.attempts.first().expect("attempt");
    assert_eq!(attempt.submitted_at, t(4));
    assert_eq!(
        attempt.outcome,
        dpm_model::AttemptOutcome::Verified {
            actor: reviewer(),
            at: t(6)
        }
    );
    let recorded = serde_json::to_value(&started).expect("json");
    assert_eq!(
        recorded["command"]["Start"]["occurred_at"],
        serde_json::json!(t(2))
    );
    assert_eq!(recorded["timestamp"], serde_json::json!(t(10)));
}

#[test]
fn an_absent_occurrence_time_serializes_and_replays_as_before() {
    let (mut plan, a, _) = pair(DependencyKind::FinishStart, 0.0, DependencyPolicy::Hard);
    let mut replayed = plan.clone();
    ok(&mut plan, &worker(), Command::Claim { work: a }, 0);
    let operation = commit(&mut plan, &worker(), start(a), 1);
    let json = serde_json::to_string(&operation.command).expect("json");
    assert_eq!(json, format!(r#"{{"Start":{{"work":"{a}"}}}}"#));
    let backfilled = commit(&mut plan, &worker(), submit_at(a, 1), 3);
    // A logged operation replays from its serialized form to the same state.
    ok(&mut replayed, &worker(), Command::Claim { work: a }, 0);
    for recorded in [operation, backfilled] {
        let text = serde_json::to_string(&recorded).expect("json");
        let decoded: Operation = serde_json::from_str(&text).expect("decode");
        apply_command(
            &mut replayed,
            decoded.actor,
            decoded.command,
            decoded.timestamp,
            decoded.id,
        )
        .expect("replay");
    }
    assert_eq!(replayed, plan);
    assert_eq!(
        plan.work_items[&a].execution.events.submitted_at,
        Some(t(1))
    );
}

#[test]
fn an_occurrence_after_the_commit_is_refused_with_both_times() {
    let (mut plan, a, _) = pair(DependencyKind::FinishStart, 0.0, DependencyPolicy::Hard);
    ok(&mut plan, &worker(), Command::Claim { work: a }, 0);
    let error = refused_at(&mut plan, &worker(), start_at(a, 11), 10);
    let EngineError::OccurrenceInFuture {
        work,
        key,
        transition,
        occurred_at,
        committed_at,
    } = &error
    else {
        panic!("expected a future occurrence, got {error:?}");
    };
    assert_eq!(
        (
            *work,
            key.to_string().as_str(),
            *transition,
            *occurred_at,
            *committed_at
        ),
        (a, "TEST-A", Transition::Start, t(11), t(10))
    );
    let message = error.to_string();
    assert!(message.contains("TEST-A") && message.contains("2026-09-01 11:00:00 UTC"));
    ok(&mut plan, &worker(), start_at(a, 10), 10);
}

fn before_previous(error: &EngineError) -> (Transition, DateTime<Utc>, DateTime<Utc>) {
    let EngineError::OccurrenceBeforePrevious {
        transition,
        occurred_at,
        previous_at,
        ..
    } = error
    else {
        panic!("expected an occurrence before the previous event, got {error:?}");
    };
    (*transition, *occurred_at, *previous_at)
}

#[test]
fn an_occurrence_before_the_previous_recorded_event_is_refused() {
    let (mut plan, a, _) = pair(DependencyKind::FinishStart, 0.0, DependencyPolicy::Hard);
    let finisher = ActorId::agent("finisher");
    ok(&mut plan, &worker(), Command::Claim { work: a }, 0);
    let handoff = Command::Handoff {
        work: a,
        from: worker(),
        to: finisher.clone(),
        reason: "reassigned".into(),
    };
    ok(&mut plan, &ActorId::human("lead"), handoff, 5);
    // The handoff is the latest recorded time, since a claim records none.
    let error = refused_at(&mut plan, &finisher, start_at(a, 4), 10);
    assert_eq!(before_previous(&error), (Transition::Start, t(4), t(5)));
    ok(&mut plan, &finisher, start_at(a, 5), 10);
    let error = refused_at(&mut plan, &finisher, submit_at(a, 4), 10);
    assert_eq!(before_previous(&error), (Transition::Submit, t(4), t(5)));
    ok(&mut plan, &finisher, submit_at(a, 7), 10);
    let reject = Command::Reject {
        work: a,
        reason: "missing test".into(),
    };
    ok(&mut plan, &reviewer(), reject, 12);
    let error = refused_at(&mut plan, &finisher, submit_at(a, 11), 13);
    assert_eq!(before_previous(&error), (Transition::Submit, t(11), t(12)));
    ok(&mut plan, &finisher, submit_at(a, 12), 13);
    let error = refused_at(&mut plan, &reviewer(), verify_at(a, 11), 14);
    assert_eq!(before_previous(&error), (Transition::Verify, t(11), t(12)));
    ok(&mut plan, &reviewer(), verify_at(a, 13), 14);
    assert_eq!(
        plan.work_items[&a].execution.events.verified_at,
        Some(t(13))
    );
}

#[test]
fn gates_are_evaluated_at_the_backfilled_time() {
    let (mut plan, a, b) = pair(DependencyKind::StartStart, 4.0, DependencyPolicy::Hard);
    let other = ActorId::agent("other");
    ok(&mut plan, &worker(), Command::Claim { work: a }, 0);
    ok(&mut plan, &worker(), start_at(a, 1), 3);
    // The lag counts from the backfilled start, so the successor opens at +5h, not +7h.
    ok(&mut plan, &other, Command::Claim { work: b }, 6);
    let EngineError::NotReady { transition, .. } = refused_at(&mut plan, &other, start_at(b, 4), 6)
    else {
        panic!("a start before the lag elapsed must be refused by its gate");
    };
    assert_eq!(transition, Transition::Start);
    ok(&mut plan, &other, start_at(b, 5), 6);
    assert_eq!(plan.work_items[&b].execution.events.started_at, Some(t(5)));
}

#[test]
fn a_start_backfilled_before_its_predecessor_finished_is_refused() {
    let (mut plan, a, b) = pair(DependencyKind::FinishStart, 0.0, DependencyPolicy::Hard);
    let other = ActorId::agent("other");
    ok(&mut plan, &worker(), Command::Claim { work: a }, 0);
    ok(&mut plan, &worker(), start(a), 1);
    ok(&mut plan, &worker(), submit(a), 2);
    ok(&mut plan, &reviewer(), verify(a), 8);
    ok(&mut plan, &other, Command::Claim { work: b }, 9);
    let error = refused_at(&mut plan, &other, start_at(b, 7), 9);
    assert!(matches!(error, EngineError::NotReady { .. }), "{error:?}");
    ok(&mut plan, &other, start_at(b, 8), 9);
}

#[test]
fn a_provisional_basis_is_recorded_at_the_backfilled_start() {
    let (mut plan, a, b) = pair(DependencyKind::FinishStart, 0.0, DependencyPolicy::Hard);
    if let Some(edge) = plan.dependencies.first_mut() {
        edge.start_basis = StartBasis::Provisional;
    }
    plan.validate().expect("provisional pair");
    let other = ActorId::agent("other");
    ok(&mut plan, &worker(), Command::Claim { work: a }, 0);
    ok(&mut plan, &worker(), start(a), 1);
    ok(&mut plan, &worker(), submit(a), 2);
    ok(&mut plan, &other, Command::Claim { work: b }, 3);
    ok(&mut plan, &other, start_at(b, 3), 5);
    let basis = plan.work_items[&b].execution.basis.first().expect("basis");
    assert_eq!((basis.attempt, basis.recorded_at), (1, t(3)));
}
