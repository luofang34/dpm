//! Conditional work and branch joins through the public command and query contract.
#![cfg(test)]

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{
    Command, EngineError, Transition, UnmetGate, apply_command, gate_report, progress,
    propose_change,
};
use dpm_model::{
    ActorId, Applicability, DecisionId, DecisionStatus, EventTime, JoinPolicy, Key, Plan,
    WorkItemId, WorkStatus,
};

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

fn supplier(plan: &Plan) -> DecisionId {
    plan.find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id
}

fn worker() -> ActorId {
    ActorId::agent("worker")
}

fn lead() -> ActorId {
    ActorId::human("lead")
}

fn run(plan: &mut Plan, actor: ActorId, command: Command, at: DateTime<Utc>) {
    apply_command(plan, actor, command, at, dpm_model::OperationId::new()).expect("command");
}

fn decide(plan: &mut Plan, option: &str, at: DateTime<Utc>) {
    let decision = supplier(plan);
    let outcome = option.into();
    run(plan, lead(), Command::Decide { decision, outcome }, at);
}

fn complete(plan: &mut Plan, key: &str, at: DateTime<Utc>) {
    let work = id(plan, key);
    for command in [
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
    ] {
        run(plan, worker(), command, at);
    }
    run(
        plan,
        lead(),
        Command::Verify {
            work,
            note: None,
            occurred_at: None,
        },
        at,
    );
}

/// A refused command reports the applicability gate and changes nothing, not even the revision.
fn refused(plan: &mut Plan, key: &str, command: Command) -> Vec<UnmetGate> {
    let before = plan.clone();
    let error = apply_command(
        plan,
        worker(),
        command,
        t(50),
        dpm_model::OperationId::new(),
    )
    .expect_err(key);
    assert_eq!(*plan, before, "{key}: failed commands are atomic");
    match error {
        EngineError::NotReady { unmet, .. } => {
            assert!(
                unmet
                    .iter()
                    .any(|g| matches!(g, UnmetGate::Applicability { .. })),
                "{key}: {unmet:?}"
            );
            unmet
        }
        other => panic!("{key}: unexpected {other}"),
    }
}

#[test]
fn supplier_a_happy_path_joins_only_the_selected_verified_branch() {
    let mut plan = fixture();
    decide(&mut plan, "A", t(1));
    complete(&mut plan, "SUP-DESIGN", t(2));
    complete(&mut plan, "SUP-A-QUOTE", t(3));
    let merge = id(&plan, "SUP-MERGE");
    let build = id(&plan, "SUP-BUILD");
    assert!(
        !gate_report(&plan, build, Transition::Claim, t(4))
            .expect("gate")
            .ready
    );
    complete(&mut plan, "SUP-A-QUAL", t(5));
    let report = progress(&plan, t(6)).expect("progress");
    assert!(report.work[&merge].verified);
    assert_eq!(
        report.work[&merge].completed_at,
        Some(EventTime::Recorded(t(5)))
    );
    assert!(
        gate_report(&plan, build, Transition::Claim, t(6))
            .expect("gate")
            .ready
    );

    // The unselected branch cannot be claimed, and the refusal changes no state.
    let b_quote = id(&plan, "SUP-B-QUOTE");
    refused(&mut plan, "SUP-B-QUOTE", Command::Claim { work: b_quote });
    complete(&mut plan, "SUP-BUILD", t(7));
    complete(&mut plan, "SUP-A-AUDIT", t(7));
    let report = progress(&plan, t(8)).expect("progress");
    assert!(report.overall.verified, "every applicable item is complete");
    assert_eq!(report.overall.percent_complete, 100.0);
    assert!(!report.work[&id(&plan, "SUP-PKG-B")].verified);
}

#[test]
fn an_unknown_choice_is_not_completion_and_its_branches_cannot_be_claimed() {
    let mut plan = fixture();
    complete(&mut plan, "SUP-DESIGN", t(1));
    let quote = id(&plan, "SUP-A-QUOTE");
    let unmet = refused(&mut plan, "SUP-A-QUOTE", Command::Claim { work: quote });
    assert!(unmet.iter().any(|g| matches!(
        g,
        UnmetGate::Applicability {
            applicability: Applicability::Undecided { .. }
        }
    )));
    let report = progress(&plan, t(2)).expect("progress");
    assert!(!report.work[&id(&plan, "SUP-MERGE")].verified);
    assert!(!report.overall.verified);
    let build = id(&plan, "SUP-BUILD");
    let gate = gate_report(&plan, build, Transition::Claim, t(2)).expect("gate");
    assert!(gate.unmet.iter().any(|g| matches!(
        g,
        UnmetGate::Applicability {
            applicability: Applicability::AwaitingChoice { .. }
        }
    )));
}

#[test]
fn a_skipped_predecessor_does_not_release_ordinary_downstream_work() {
    let mut plan = fixture();
    decide(&mut plan, "B", t(1));
    complete(&mut plan, "SUP-DESIGN", t(2));
    let audit = id(&plan, "SUP-A-AUDIT");
    let unmet = refused(&mut plan, "SUP-A-AUDIT", Command::Claim { work: audit });
    assert!(unmet.iter().any(|g| matches!(
        g,
        UnmetGate::Dependency {
            release: dpm_model::Release::NotSelected,
            ..
        }
    )));
    // Nested packages pass the exclusion to their tasks.
    let qualification = id(&plan, "SUP-A-QUAL");
    refused(
        &mut plan,
        "SUP-A-QUAL",
        Command::Claim {
            work: qualification,
        },
    );
    complete(&mut plan, "SUP-B-QUOTE", t(3));
    complete(&mut plan, "SUP-B-QUAL", t(4));
    let report = progress(&plan, t(5)).expect("progress");
    assert!(report.work[&id(&plan, "SUP-MERGE")].verified);
    assert!(
        !report.work[&audit].verified,
        "stranded work is outstanding, not complete"
    );
}

#[test]
fn an_all_skipped_join_needs_an_explicit_empty_permission() {
    let mut plan = fixture();
    let package = plan.find_work_by_key_mut("SUP-PKG-B").expect("package");
    package
        .contract
        .condition
        .as_mut()
        .expect("condition")
        .option = "A".into();
    let mut permissive = plan.clone();
    permissive
        .find_work_by_key_mut("SUP-MERGE")
        .expect("merge")
        .contract
        .join = JoinPolicy::ActiveBranches { allow_empty: true };

    decide(&mut plan, "B", t(1));
    let merge = id(&plan, "SUP-MERGE");
    let gate = gate_report(&plan, id(&plan, "SUP-BUILD"), Transition::Claim, t(2)).expect("gate");
    assert!(!gate.ready);
    assert!(!progress(&plan, t(2)).expect("progress").work[&merge].verified);
    let explained = dpm_engine::explain_work(&plan, merge, t(2)).expect("explain");
    assert_eq!(explained.applicability, Applicability::EmptyJoin);

    decide(&mut permissive, "B", t(1));
    let report = progress(&permissive, t(2)).expect("progress");
    assert_eq!(
        report.work[&merge].completed_at,
        Some(EventTime::Recorded(t(1)))
    );
    let build = id(&permissive, "SUP-BUILD");
    assert!(
        gate_report(&permissive, build, Transition::Claim, t(2))
            .expect("gate")
            .ready
    );
}

#[test]
fn decide_requires_a_declared_option_atomically() {
    let mut plan = fixture();
    let before = plan.clone();
    let decision = supplier(&plan);
    let error = apply_command(
        &mut plan,
        lead(),
        Command::Decide {
            decision,
            outcome: "Supplier A".into(),
        },
        t(1),
        dpm_model::OperationId::new(),
    )
    .expect_err("undeclared option");
    assert!(
        error
            .to_string()
            .contains("must be one of the option keys A, B")
    );
    assert_eq!(plan, before);
}

#[test]
fn a_choice_that_would_exclude_started_work_is_refused_by_decide() {
    // Reachable only through state that predates the condition; decide must still not cancel it.
    let mut plan = fixture();
    let quote = plan.find_work_by_key_mut("SUP-B-QUOTE").expect("work");
    quote.execution.status = WorkStatus::InProgress;
    quote.execution.owner = Some(worker());
    quote.execution.events.started_at = Some(t(0));
    plan.validate().expect("valid legacy state");
    let before = plan.clone();
    let decision = supplier(&plan);
    let outcome = "A".into();
    let error = apply_command(
        &mut plan,
        lead(),
        Command::Decide { decision, outcome },
        t(1),
        dpm_model::OperationId::new(),
    )
    .expect_err("excludes in-flight work");
    assert!(error.to_string().contains("in-flight work SUP-B-QUOTE"));
    assert_eq!(plan, before);
    decide(&mut plan, "B", t(1));
}

#[test]
fn changing_a_choice_after_work_starts_requires_a_reviewed_change_and_keeps_the_work() {
    let mut plan = fixture();
    decide(&mut plan, "A", t(1));
    complete(&mut plan, "SUP-DESIGN", t(2));
    let quote = id(&plan, "SUP-A-QUOTE");
    run(&mut plan, worker(), Command::Claim { work: quote }, t(3));
    run(
        &mut plan,
        worker(),
        Command::Start {
            work: quote,
            occurred_at: None,
        },
        t(3),
    );
    let decision = supplier(&plan);
    let again = apply_command(
        &mut plan.clone(),
        lead(),
        Command::Decide {
            decision,
            outcome: "B".into(),
        },
        t(4),
        dpm_model::OperationId::new(),
    );
    assert!(matches!(again, Err(EngineError::DecisionNotOpen(_))));

    let proposed = switch_to_b(&plan);
    let preview = propose_change(&plan, &proposed).expect("reviewable");
    let change = preview
        .applicability_changes
        .iter()
        .find(|c| c.key.0 == "SUP-A-QUOTE")
        .expect("in-flight work is reported");
    assert!(change.in_flight);
    assert!(matches!(
        change.after,
        Some(Applicability::NotSelected { .. })
    ));
    let agent = dpm_engine::apply_plan_change(
        &mut plan.clone(),
        worker(),
        &proposed,
        "Supplier A withdrew",
        t(4),
        dpm_model::OperationId::new(),
    );
    assert!(matches!(agent, Err(EngineError::ActorNotAllowed { .. })));
    let command = dpm_engine::plan_change(&plan, &proposed, "Supplier A withdrew").expect("delta");
    run(&mut plan, lead(), command, t(4));
    let work = &plan.work_items[&quote];
    assert_eq!(
        work.execution.status,
        WorkStatus::InProgress,
        "not cancelled"
    );
    assert_eq!(work.execution.owner, Some(worker()));
    refused(
        &mut plan,
        "SUP-A-QUOTE",
        Command::Submit {
            work: quote,
            note: None,
            occurred_at: None,
        },
    );
    assert!(
        gate_report(&plan, id(&plan, "SUP-B-QUOTE"), Transition::Claim, t(5))
            .expect("gate")
            .ready
    );
}

#[test]
fn review_protects_conditions_of_started_work() {
    let mut plan = fixture();
    decide(&mut plan, "A", t(1));
    complete(&mut plan, "SUP-DESIGN", t(2));
    let mut proposed = plan.clone();
    proposed
        .find_work_by_key_mut("SUP-DESIGN")
        .expect("work")
        .contract
        .condition = plan
        .find_work_by_key("SUP-PKG-A")
        .expect("package")
        .contract
        .condition
        .clone();
    assert!(propose_change(&plan, &proposed).is_err());

    // Unstarted work may gain a condition; the diff names the field.
    let mut proposed = plan.clone();
    proposed
        .find_work_by_key_mut("SUP-BUILD")
        .expect("work")
        .contract
        .join = JoinPolicy::ActiveBranches { allow_empty: true };
    let preview = propose_change(&plan, &proposed).expect("allowed");
    assert_eq!(preview.changes[0].fields, vec!["contract".to_string()]);
}

/// A reviewed replacement that selects supplier B, keeping the same option keys.
fn switch_to_b(plan: &Plan) -> Plan {
    let mut proposed = plan.clone();
    let old = proposed
        .decisions
        .get_mut(&supplier(plan))
        .expect("decision");
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
