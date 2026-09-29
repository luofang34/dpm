//! Work packages whose children a choice excluded, through the public command and query contract.
#![cfg(test)]

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{Command, ProgressScope, apply_command, explain_work, progress, status};
use dpm_model::{Applicability, Key, Plan, WorkCondition, WorkItemId};

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

fn run(plan: &mut Plan, actor: &str, command: Command, at: DateTime<Utc>) {
    let actor = if actor == "lead" {
        dpm_model::ActorId::human(actor)
    } else {
        dpm_model::ActorId::agent(actor)
    };
    apply_command(plan, actor, command, at, dpm_model::OperationId::new()).expect("command");
}

fn complete(plan: &mut Plan, key: &str, at: DateTime<Utc>) {
    let work = id(plan, key);
    for command in [
        Command::Claim { work },
        Command::Start { work },
        Command::Submit { work, note: None },
    ] {
        run(plan, "worker", command, at);
    }
    run(plan, "lead", Command::Verify { work, note: None }, at);
}

/// The fixture plus an unconditional package whose only task applies only to supplier A, without
/// the audit that an ordinary dependency on supplier A work strands.
fn with_supplier_a_package() -> Plan {
    let mut plan = fixture();
    let audit = id(&plan, "SUP-A-AUDIT");
    plan.work_items.remove(&audit);
    plan.dependencies
        .retain(|d| d.predecessor != audit && d.successor != audit);
    let decision = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let mut package = plan.find_work_by_key("SUP-PKG-A").expect("package").clone();
    package.id = WorkItemId::new();
    package.key = Key::new("X-PKG");
    package.contract.condition = None;
    let mut task = plan.find_work_by_key("SUP-A-QUOTE").expect("task").clone();
    task.id = WorkItemId::new();
    task.key = Key::new("X-A1");
    task.parent = Some(package.id);
    task.contract.condition = Some(WorkCondition {
        decision,
        option: "A".into(),
    });
    plan.work_items.insert(package.id, package);
    plan.work_items.insert(task.id, task);
    plan.validate().expect("valid plan");
    plan
}

#[test]
fn a_package_of_only_unselected_work_is_excluded_in_every_view_and_does_not_block_completion() {
    let mut plan = with_supplier_a_package();
    let decision = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let outcome = "B".into();
    run(
        &mut plan,
        "lead",
        Command::Decide { decision, outcome },
        t(1),
    );
    for (key, hours) in [
        ("SUP-DESIGN", 2),
        ("SUP-B-QUOTE", 3),
        ("SUP-B-QUAL", 4),
        ("SUP-BUILD", 5),
    ] {
        complete(&mut plan, key, t(hours));
    }
    let package = id(&plan, "X-PKG");
    let excluded = Applicability::AllChildrenExcluded {
        decisions: vec![Key::new("DEC-SUPPLIER")],
    };

    let explained = explain_work(&plan, package, t(6)).expect("explain");
    assert_eq!(explained.applicability, excluded);
    let summary = status(&plan, false, t(6)).expect("status");
    assert!(
        summary
            .not_applicable
            .iter()
            .any(|w| w.key.0 == "X-PKG" && w.applicability == excluded),
        "{:?}",
        summary.not_applicable
    );
    let report = progress(&plan, t(6)).expect("progress");
    assert_eq!(report.work[&package].scope, ProgressScope::NotSelected);
    assert!(!report.work[&package].verified, "excluded is not completed");
    assert!(
        report.overall.verified,
        "the excluded package must not hold the workspace open"
    );
    assert!((report.overall.percent_complete - 100.0).abs() < f64::EPSILON);
}

#[test]
fn an_open_choice_keeps_the_package_uncommitted_and_the_workspace_open() {
    let mut plan = with_supplier_a_package();
    complete(&mut plan, "SUP-DESIGN", t(2));
    let package = id(&plan, "X-PKG");
    let awaiting = Applicability::AwaitingChoice {
        predecessor: Key::new("X-A1"),
    };
    assert_eq!(
        explain_work(&plan, package, t(3))
            .expect("explain")
            .applicability,
        awaiting
    );
    let summary = status(&plan, false, t(3)).expect("status");
    assert!(
        summary
            .not_applicable
            .iter()
            .any(|w| w.key.0 == "X-PKG" && w.applicability == awaiting)
    );
    let report = progress(&plan, t(3)).expect("progress");
    assert!(!report.work[&package].verified && !report.overall.verified);
}
