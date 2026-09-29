//! A reviewed replacement of a made choice, observed through the public command and query API.
#![cfg(test)]

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{Command, Transition, apply_command, gate_report, progress, status};
use dpm_model::{ActorId, DecisionId, DecisionStatus, EventTime, Key, Plan, WorkItemId};

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

fn run(plan: &mut Plan, actor: &ActorId, command: Command, at: DateTime<Utc>) {
    let label = format!("{command:?}");
    apply_command(
        plan,
        actor.clone(),
        command,
        at,
        dpm_model::OperationId::new(),
    )
    .unwrap_or_else(|e| panic!("{label}: {e}"));
}

fn worker() -> ActorId {
    ActorId::agent("worker")
}

fn lead() -> ActorId {
    ActorId::human("lead")
}

fn complete(plan: &mut Plan, key: &str, at: DateTime<Utc>) {
    let work = id(plan, key);
    run(plan, &worker(), Command::Claim { work }, at);
    run(plan, &worker(), Command::Start { work }, at);
    run(plan, &worker(), Command::Submit { work, note: None }, at);
    run(plan, &lead(), Command::Verify { work, note: None }, at);
}

/// Supplier A is chosen at +1h and its quote is started at +3h; the lead switches to B at +4h.
/// SUP-BUILD waits one hour after the SUP-MERGE join.
fn switched_under_started_work() -> (Plan, DecisionId) {
    let mut plan = fixture();
    let (merge, build) = (id(&plan, "SUP-MERGE"), id(&plan, "SUP-BUILD"));
    plan.dependencies
        .iter_mut()
        .find(|d| d.predecessor == merge && d.successor == build)
        .expect("merge edge")
        .lag_hours = 1.0;
    let supplier = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let decide = Command::Decide {
        decision: supplier,
        outcome: "A".into(),
    };
    run(&mut plan, &lead(), decide, t(1));
    complete(&mut plan, "SUP-DESIGN", t(2));
    let quote = id(&plan, "SUP-A-QUOTE");
    run(&mut plan, &worker(), Command::Claim { work: quote }, t(3));
    run(&mut plan, &worker(), Command::Start { work: quote }, t(3));
    let proposed = switch_to_b(&plan, supplier);
    let replacement_id = *proposed
        .decisions
        .keys()
        .find(|d| !plan.decisions.contains_key(d))
        .expect("replacement");
    let change = dpm_engine::plan_change(&plan, &proposed, "Supplier A withdrew").expect("delta");
    run(&mut plan, &lead(), change, t(4));
    (plan, replacement_id)
}

/// A reviewed replacement selecting supplier B; like any proposal it carries no event time.
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
fn a_replacement_is_resolved_at_its_apply_time_so_lag_from_it_elapses() {
    let (mut plan, replacement) = switched_under_started_work();
    complete(&mut plan, "SUP-B-QUOTE", t(5));
    complete(&mut plan, "SUP-B-QUAL", t(6));
    let merge = id(&plan, "SUP-MERGE");
    let merged = progress(&plan, t(6)).expect("progress").work[&merge].completed_at;
    assert_eq!(merged, Some(EventTime::Recorded(t(6))));
    let build = id(&plan, "SUP-BUILD");
    let claim = |hour| {
        gate_report(&plan, build, Transition::Claim, t(hour))
            .expect("gates")
            .ready
    };
    assert!(
        !claim(6),
        "the hour of lag after the join is still elapsing"
    );
    assert!(claim(7), "lag from a replacement-dependent join elapses");
    let resolved = plan.decisions[&replacement].resolved_at;
    assert_eq!(resolved, Some(t(4)), "the apply command's own time");
}

#[test]
fn a_proposal_still_cannot_author_a_replacement_resolution_time() {
    let mut plan = fixture();
    let supplier = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let decide = Command::Decide {
        decision: supplier,
        outcome: "A".into(),
    };
    run(&mut plan, &lead(), decide, t(1));
    let mut proposed = switch_to_b(&plan, supplier);
    for decision in proposed.decisions.values_mut() {
        if decision.supersedes == Some(supplier) {
            decision.resolved_at = Some(t(0));
        }
    }
    let before = plan.clone();
    assert!(
        dpm_engine::apply_plan_change(
            &mut plan,
            lead(),
            &proposed,
            "backdated",
            t(2),
            dpm_model::OperationId::new()
        )
        .is_err()
    );
    assert_eq!(plan, before);
}

#[test]
fn status_counts_only_in_flight_work_that_progress_counts() {
    let (plan, _) = switched_under_started_work();
    let summary = status(&plan, false, t(5)).expect("status");
    let quote = id(&plan, "SUP-A-QUOTE");
    let scope = progress(&plan, t(5)).expect("progress").work[&quote].scope;
    assert_eq!(
        serde_json::to_value(scope).expect("scope"),
        "not_selected",
        "progress excludes the switched-away quote"
    );
    assert_eq!(
        (summary.in_flight, summary.excluded_in_flight),
        (0, 1),
        "the excluded quote is reported apart from counted in-flight work"
    );
    assert_eq!(summary.blocked + summary.awaiting_verification, 0);
}

/// Supplier A is chosen at +1h and its branch reaches SUP-MERGE at +4h; SUP-BUILD waits three
/// hours after the join and is claimed at +8h.
fn merged_and_claimed() -> (Plan, DecisionId) {
    let mut plan = fixture();
    let (merge, build) = (id(&plan, "SUP-MERGE"), id(&plan, "SUP-BUILD"));
    plan.dependencies
        .iter_mut()
        .find(|d| d.predecessor == merge && d.successor == build)
        .expect("merge edge")
        .lag_hours = 3.0;
    let supplier = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let decide = Command::Decide {
        decision: supplier,
        outcome: "A".into(),
    };
    run(&mut plan, &lead(), decide, t(1));
    complete(&mut plan, "SUP-DESIGN", t(2));
    complete(&mut plan, "SUP-A-QUOTE", t(3));
    complete(&mut plan, "SUP-A-QUAL", t(4));
    run(&mut plan, &worker(), Command::Claim { work: build }, t(8));
    (plan, supplier)
}

#[test]
fn reaffirming_a_choice_does_not_move_a_reached_join_or_reclose_claimed_work() {
    let (mut plan, supplier) = merged_and_claimed();
    let (merge, build) = (id(&plan, "SUP-MERGE"), id(&plan, "SUP-BUILD"));
    let reached = progress(&plan, t(8)).expect("progress").work[&merge].completed_at;
    assert_eq!(reached, Some(EventTime::Recorded(t(4))));
    let mut proposed = switch_to_b(&plan, supplier);
    for decision in proposed.decisions.values_mut() {
        if decision.supersedes == Some(supplier) {
            decision.outcome = Some("A".into());
            decision.rationale = Some("Supplier A renewed its quote".into());
        }
    }
    let change = dpm_engine::plan_change(&plan, &proposed, "reaffirm supplier A").expect("delta");
    run(&mut plan, &lead(), change, t(9));
    let after = progress(&plan, t(9)).expect("progress").work[&merge].completed_at;
    assert_eq!(
        after, reached,
        "the unchanged outcome has stood since +1h, so the skipped branch released then"
    );
    let start = gate_report(&plan, build, Transition::Start, t(9)).expect("gates");
    assert!(
        start.ready,
        "a released start gate stays released: {:?}",
        start.reasons()
    );
}
