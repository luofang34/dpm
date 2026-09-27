//! A reviewed replacement of a made choice, observed through the public command and query API.
#![allow(clippy::expect_used, clippy::panic)]

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{Command, apply_command, progress, status};
use dpm_model::{ActorId, DecisionId, DecisionStatus, Key, Plan, WorkItemId};

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
    apply_command(plan, actor.clone(), command, at).unwrap_or_else(|e| panic!("{label}: {e}"));
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
fn switched_under_started_work() -> (Plan, DecisionId) {
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
    complete(&mut plan, "SUP-DESIGN", t(2));
    let quote = id(&plan, "SUP-A-QUOTE");
    run(&mut plan, &worker(), Command::Claim { work: quote }, t(3));
    run(&mut plan, &worker(), Command::Start { work: quote }, t(3));
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
    let replacement_id = replacement.id;
    proposed.decisions.insert(replacement.id, replacement);
    let change = Command::ApplyChange {
        plan: Box::new(proposed),
        reason: "Supplier A withdrew".into(),
    };
    run(&mut plan, &lead(), change, t(4));
    (plan, replacement_id)
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
