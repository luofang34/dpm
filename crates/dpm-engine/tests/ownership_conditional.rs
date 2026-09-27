//! Releasing and handing off work that a reviewed replacement excluded while it was in flight.
#![allow(clippy::expect_used, clippy::panic)]

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{Command, EngineError, UnmetGate, apply_command, status};
use dpm_model::{ActorId, DecisionId, DecisionStatus, Key, Plan, WorkItemId, WorkStatus};

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

fn id(plan: &Plan, key: &str) -> WorkItemId {
    plan.find_work_by_key(key).expect("key").id
}

fn run(plan: &mut Plan, actor: &ActorId, command: Command, hour: i64) {
    let label = format!("{command:?}");
    apply_command(
        plan,
        actor.clone(),
        command,
        t(hour),
        dpm_model::OperationId::new(),
    )
    .unwrap_or_else(|e| panic!("{label}: {e}"));
}

fn refused(plan: &mut Plan, actor: &ActorId, command: Command, hour: i64) -> EngineError {
    let before = plan.clone();
    let error = apply_command(
        plan,
        actor.clone(),
        command,
        t(hour),
        dpm_model::OperationId::new(),
    )
    .expect_err("refused");
    assert_eq!(*plan, before, "a refused command changes nothing");
    error
}

fn worker() -> ActorId {
    ActorId::agent("worker")
}

fn lead() -> ActorId {
    ActorId::human("lead")
}

/// Supplier A is chosen and its quote reserved (and optionally started) before the lead switches
/// to supplier B through a reviewed replacement, which keeps the quote's lifecycle.
fn excluded_quote(started: bool) -> (Plan, WorkItemId) {
    let mut plan: Plan =
        serde_json::from_str(include_str!("../../../tests/support/conditional-plan.json"))
            .expect("conditional fixture");
    let supplier = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let decide = Command::Decide {
        decision: supplier,
        outcome: "A".into(),
    };
    run(&mut plan, &lead(), decide, 1);
    let design = id(&plan, "SUP-DESIGN");
    run(&mut plan, &worker(), Command::Claim { work: design }, 2);
    run(&mut plan, &worker(), Command::Start { work: design }, 2);
    let submit = Command::Submit {
        work: design,
        note: None,
    };
    run(&mut plan, &worker(), submit, 2);
    let verify = Command::Verify {
        work: design,
        note: None,
    };
    run(&mut plan, &lead(), verify, 2);
    let quote = id(&plan, "SUP-A-QUOTE");
    run(&mut plan, &worker(), Command::Claim { work: quote }, 3);
    if started {
        run(&mut plan, &worker(), Command::Start { work: quote }, 3);
    }
    let change =
        dpm_engine::plan_change(&plan, &switch_to_b(&plan, supplier), "Supplier A withdrew")
            .expect("delta");
    run(&mut plan, &lead(), change, 4);
    assert_eq!(
        status(&plan, false, t(4))
            .expect("status")
            .excluded_in_flight,
        1
    );
    (plan, quote)
}

fn switch_to_b(plan: &Plan, supplier: DecisionId) -> Plan {
    let mut proposed = plan.clone();
    let old = proposed.decisions.get_mut(&supplier).expect("decision");
    old.status = DecisionStatus::Superseded;
    let mut replacement = old.clone();
    replacement.id = DecisionId::new();
    replacement.key = Key::new("DEC-SUPPLIER-B");
    replacement.status = DecisionStatus::Decided;
    replacement.outcome = Some("B".into());
    replacement.resolved_at = None;
    replacement.rationale = Some("Supplier A withdrew its quote".into());
    replacement.supersedes = Some(old.id);
    proposed.decisions.insert(replacement.id, replacement);
    proposed
}

#[test]
fn an_excluded_claim_is_released_and_stops_counting_as_in_flight() {
    let (mut plan, quote) = excluded_quote(false);
    let release = Command::Release {
        work: quote,
        reason: "supplier A was dropped".into(),
    };
    run(&mut plan, &worker(), release, 5);
    let item = &plan.work_items[&quote];
    assert_eq!(
        (item.status, item.owner.clone()),
        (WorkStatus::Planned, None)
    );
    let summary = status(&plan, false, t(5)).expect("status");
    assert_eq!(summary.excluded_in_flight, 0);
    assert!(!summary.gates[&quote].ready, "excluded work is never ready");
    let claim = Command::Claim { work: quote };
    assert!(matches!(
        refused(&mut plan, &worker(), claim, 6),
        EngineError::NotReady { .. }
    ));
}

#[test]
fn excluded_started_work_changes_hands_but_still_takes_no_transition() {
    let (mut plan, quote) = excluded_quote(true);
    let release = Command::Release {
        work: quote,
        reason: "supplier A was dropped".into(),
    };
    assert!(matches!(
        refused(&mut plan, &worker(), release, 5),
        EngineError::AlreadyStarted(_)
    ));
    let other = ActorId::agent("other");
    let handoff = Command::Handoff {
        work: quote,
        from: worker(),
        to: other.clone(),
        reason: "worker reassigned while the plan is reviewed".into(),
    };
    run(&mut plan, &lead(), handoff, 5);
    assert_eq!(plan.work_items[&quote].owner, Some(other.clone()));
    let submit = Command::Submit {
        work: quote,
        note: None,
    };
    let EngineError::NotReady { unmet, .. } = refused(&mut plan, &other, submit, 6) else {
        panic!("excluded work must stay gated");
    };
    assert!(
        unmet
            .iter()
            .any(|g| matches!(g, UnmetGate::Applicability { .. }))
    );
    assert_eq!(
        status(&plan, false, t(6))
            .expect("status")
            .excluded_in_flight,
        1
    );
}
