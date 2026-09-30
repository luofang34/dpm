//! Conditional work cannot record events from before the choice that put it in the plan.
#![cfg(test)]

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{Command, EngineError, Transition, UnmetGate, apply_command, gate_report};
use dpm_model::{ActorId, Plan, Release, Timeline, WorkItemId};

fn fixture() -> Plan {
    serde_json::from_str(include_str!("../../../tests/support/conditional-plan.json"))
        .expect("conditional fixture")
}

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

fn id(plan: &Plan, key: &str) -> WorkItemId {
    plan.find_work_by_key(key).expect("key").id
}

fn worker() -> ActorId {
    ActorId::agent("worker")
}

fn lead() -> ActorId {
    ActorId::human("lead")
}

fn apply(
    plan: &mut Plan,
    actor: ActorId,
    command: Command,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    apply_command(plan, actor, command, at, dpm_model::OperationId::new()).map(|_| ())
}

fn run(plan: &mut Plan, actor: ActorId, command: Command, at: DateTime<Utc>) {
    apply(plan, actor, command, at).expect("command");
}

/// SUP-DESIGN verified at +1h and DEC-SUPPLIER deciding A at +10h.
fn chosen_at_ten() -> Plan {
    let mut plan = fixture();
    let work = id(&plan, "SUP-DESIGN");
    let steps = [
        Command::Claim { work },
        Command::Start {
            work,
            occurred_at: None,
        },
        Command::Submit {
            work,
            note: None,
            occurred_at: None,
        },
    ];
    for command in steps {
        run(&mut plan, worker(), command, t(1));
    }
    let verify = Command::Verify {
        work,
        note: None,
        occurred_at: None,
    };
    run(&mut plan, lead(), verify, t(1));
    let decision = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let outcome = "A".into();
    run(
        &mut plan,
        lead(),
        Command::Decide { decision, outcome },
        t(10),
    );
    plan
}

fn choice_gates(plan: &Plan, work: WorkItemId, transition: Transition, at: i64) -> Vec<UnmetGate> {
    gate_report(plan, work, transition, t(at))
        .expect("report")
        .unmet
        .into_iter()
        .filter(|gate| matches!(gate, UnmetGate::Choice { .. }))
        .collect()
}

#[test]
fn every_transition_of_selected_work_waits_for_the_choice_that_selected_it() {
    let plan = chosen_at_ten();
    let quote = id(&plan, "SUP-A-QUOTE");
    let expected = UnmetGate::Choice {
        key: "DEC-SUPPLIER".into(),
        chosen_at: t(10),
    };
    for transition in Transition::ALL {
        assert_eq!(
            choice_gates(&plan, quote, transition, 2),
            std::slice::from_ref(&expected),
            "{transition}"
        );
        assert_eq!(
            choice_gates(&plan, quote, transition, 10),
            [],
            "{transition}"
        );
    }
    // The condition sits on the containing package; nested work inherits the choice time.
    let qualification = id(&plan, "SUP-A-QUAL");
    assert_eq!(
        choice_gates(&plan, qualification, Transition::Start, 9),
        [expected]
    );
    let report = gate_report(&plan, quote, Transition::Claim, t(10)).expect("report");
    assert!(report.ready, "{:?}", report.unmet);
}

#[test]
fn selected_work_cannot_be_backfilled_before_its_choice() {
    let mut plan = chosen_at_ten();
    let work = id(&plan, "SUP-A-QUOTE");
    run(&mut plan, worker(), Command::Claim { work }, t(11));
    let backdated = |hour| Command::Start {
        work,
        occurred_at: Some(t(hour)),
    };
    let before = plan.clone();
    let error = apply(&mut plan, worker(), backdated(2), t(12)).expect_err("before the claim");
    assert!(
        matches!(error, EngineError::OccurrenceBeforePrevious { .. }),
        "{error:?}"
    );
    assert_eq!(plan, before);
    // A claim held without a recorded time still cannot reach back before the choice.
    if let Some(item) = plan.work_items.get_mut(&work) {
        item.execution.events.claimed_at = None;
    }
    let error = apply(&mut plan, worker(), backdated(2), t(12)).expect_err("before the choice");
    let EngineError::NotReady { unmet, .. } = error else {
        panic!("expected the choice gate, got {error:?}");
    };
    assert_eq!(
        unmet,
        [UnmetGate::Choice {
            key: "DEC-SUPPLIER".into(),
            chosen_at: t(10),
        }]
    );
    run(&mut plan, worker(), backdated(10), t(12));
}

#[test]
fn an_excluded_branch_is_skipped_only_once_the_choice_was_made() {
    let plan = chosen_at_ten();
    let (excluded, join) = (id(&plan, "SUP-B-QUAL"), id(&plan, "SUP-MERGE"));
    let edge = plan
        .dependencies
        .iter()
        .find(|d| d.predecessor == excluded && d.successor == join)
        .expect("branch edge");
    assert_eq!(
        Timeline::at(&plan, t(9)).edge(&plan, edge),
        Release::AwaitingEvent
    );
    assert_eq!(
        Timeline::at(&plan, t(10)).edge(&plan, edge),
        Release::SkippedBranch {
            at: dpm_model::EventTime::Recorded(t(10))
        }
    );
}
