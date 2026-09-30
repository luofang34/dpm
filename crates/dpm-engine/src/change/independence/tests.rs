use super::{EdgeRelaxation, RelaxedConstraint};
use crate::{Command, EngineError, Operation, UnmetGate, apply_command};
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_model::{
    ActorId, Applicability, DecisionId, DecisionStatus, Dependency, DependencyId, DependencyKind,
    DependencyPolicy, JoinPolicy, Key, Plan, StartBasis, WorkItemId,
};

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

fn id(plan: &Plan, key: &str) -> WorkItemId {
    plan.find_work_by_key(key).expect("work").id
}

fn owner() -> ActorId {
    ActorId::human("owner")
}

fn reviewer() -> ActorId {
    ActorId::human("reviewer")
}

fn run(plan: &mut Plan, actor: ActorId, command: Command, at: DateTime<Utc>) -> Operation {
    let label = format!("{command:?}");
    apply_command(plan, actor, command, at, dpm_model::OperationId::new())
        .unwrap_or_else(|e| panic!("{label}: {e}"))
}

fn apply(plan: &mut Plan, actor: ActorId, proposal: Plan) -> Result<Operation, EngineError> {
    crate::apply_plan_change(
        plan,
        actor,
        &proposal,
        "reviewed",
        t(5),
        dpm_model::OperationId::new(),
    )
}

fn edge<'a>(plan: &'a mut Plan, from: &str, to: &str) -> &'a mut Dependency {
    let (a, b) = (id(plan, from), id(plan, to));
    plan.dependencies
        .iter_mut()
        .find(|d| d.predecessor == a && d.successor == b)
        .expect("edge")
}

/// TEST-A -FS 24h Hard-> TEST-B, with TEST-A claimed by a human owner.
fn owned_predecessor() -> (Plan, DependencyId) {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let mut proposal = plan.clone();
    edge(&mut proposal, "TEST-A", "TEST-B").lag_hours = 24.0;
    apply(&mut plan, reviewer(), proposal).expect("reviewer sets the lag");
    let work = id(&plan, "TEST-A");
    run(&mut plan, owner(), Command::Claim { work }, t(1));
    let dependency = edge(&mut plan, "TEST-A", "TEST-B").id;
    (plan, dependency)
}

fn refused_as(plan: &mut Plan, proposal: Plan) -> RelaxedConstraint {
    let before = plan.clone();
    let refused = apply(plan, owner(), proposal);
    assert_eq!(*plan, before, "a refused change leaves the plan unchanged");
    match refused {
        Err(EngineError::OwnGateRelaxed { actor, relaxed, .. }) => {
            assert_eq!(actor, owner());
            *relaxed
        }
        other => panic!("expected an own-gate refusal, got {other:?}"),
    }
}

#[test]
fn an_owner_cannot_remove_or_weaken_an_edge_out_of_its_own_work() {
    let (mut plan, dependency) = owned_predecessor();
    type Edit = fn(&mut Plan);
    let edits: [(EdgeRelaxation, Edit); 6] = [
        (EdgeRelaxation::Removed, |p| {
            let b = id(p, "TEST-B");
            p.dependencies
                .retain(|d| !(d.successor == b && d.lag_hours > 0.0));
        }),
        (EdgeRelaxation::Lag, |p| {
            edge(p, "TEST-A", "TEST-B").lag_hours = 1.0;
        }),
        (EdgeRelaxation::Policy, |p| {
            edge(p, "TEST-A", "TEST-B").policy = DependencyPolicy::Soft;
        }),
        (EdgeRelaxation::Kind, |p| {
            edge(p, "TEST-A", "TEST-B").kind = DependencyKind::StartStart;
        }),
        (EdgeRelaxation::StartBasis, |p| {
            edge(p, "TEST-A", "TEST-B").start_basis = StartBasis::Provisional;
        }),
        (EdgeRelaxation::Lag, |p| {
            let replaced = edge(p, "TEST-A", "TEST-B");
            replaced.id = DependencyId::new();
            replaced.lag_hours = 2.0;
        }),
    ];
    for (expected, edit) in edits {
        let mut proposal = plan.clone();
        edit(&mut proposal);
        let relaxed = refused_as(&mut plan, proposal);
        assert_eq!(
            relaxed,
            RelaxedConstraint::Dependency {
                dependency,
                predecessor: Key::new("TEST-A"),
                successor: Key::new("TEST-B"),
                relaxation: expected,
            }
        );
    }
    let mut proposal = plan.clone();
    edge(&mut proposal, "TEST-A", "TEST-B").lag_hours = 1.0;
    apply(&mut plan, reviewer(), proposal).expect("an independent reviewer may relax it");
}

#[test]
fn an_owner_may_tighten_its_edges_or_give_one_a_new_identity() {
    let (mut plan, _) = owned_predecessor();
    let mut proposal = plan.clone();
    edge(&mut proposal, "TEST-A", "TEST-B").lag_hours = 48.0;
    edge(&mut proposal, "TEST-A", "TEST-D").rationale = Some("D consumes A".into());
    apply(&mut plan, owner(), proposal).expect("tightening");
    let mut proposal = plan.clone();
    edge(&mut proposal, "TEST-A", "TEST-B").id = DependencyId::new();
    apply(&mut plan, owner(), proposal).expect("an equally strict edge under a new identity");
    let mut proposal = plan.clone();
    let (b, c) = (id(&plan, "TEST-B"), id(&plan, "TEST-C"));
    let mut tighter = Dependency::new(id(&plan, "TEST-A"), c, DependencyKind::FinishStart, 0.0);
    tighter.id = DependencyId::new();
    proposal.dependencies.push(tighter);
    edge(&mut proposal, "TEST-B", "TEST-C").lag_hours = 0.5;
    assert_ne!(b, c);
    apply(&mut plan, owner(), proposal).expect("unrelated edges are not the owner's");
}

/// Supplier A is chosen and SUP-A-QUOTE started by the owner; the lead then switches to B, so the
/// quote is excluded in flight and SUP-A-AUDIT, an ordinary successor, is stranded behind it.
fn excluded_in_flight() -> (Plan, DecisionId) {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../../tests/support/conditional-plan.json"
    ))
    .expect("fixture");
    let supplier = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let decide = Command::Decide {
        decision: supplier,
        outcome: "A".into(),
    };
    run(&mut plan, reviewer(), decide, t(0));
    let design = id(&plan, "SUP-DESIGN");
    run(&mut plan, owner(), Command::Claim { work: design }, t(0));
    run(
        &mut plan,
        owner(),
        Command::Start {
            work: design,
            occurred_at: None,
        },
        t(0),
    );
    let submit = Command::Submit {
        work: design,
        note: None,
        occurred_at: None,
    };
    run(&mut plan, owner(), submit, t(0));
    let verify = Command::Verify {
        work: design,
        note: None,
        occurred_at: None,
    };
    run(&mut plan, reviewer(), verify, t(1));
    let quote = id(&plan, "SUP-A-QUOTE");
    run(&mut plan, owner(), Command::Claim { work: quote }, t(2));
    run(
        &mut plan,
        owner(),
        Command::Start {
            work: quote,
            occurred_at: None,
        },
        t(2),
    );
    let proposal = replace(&plan, supplier, "B");
    let replacement = *proposal
        .decisions
        .keys()
        .find(|d| !plan.decisions.contains_key(d))
        .expect("replacement");
    apply(&mut plan, reviewer(), proposal).expect("switch to B");
    (plan, replacement)
}

fn replace(plan: &Plan, standing: DecisionId, option: &str) -> Plan {
    let mut proposal = plan.clone();
    let old = proposal.decisions.get_mut(&standing).expect("decision");
    old.status = DecisionStatus::Superseded;
    let mut new = old.clone();
    new.id = DecisionId::new();
    new.key = Key::new(format!("DEC-SUPPLIER-{option}"));
    new.status = DecisionStatus::Decided;
    new.outcome = Some(option.into());
    new.resolved_at = None;
    new.rationale = Some("revisited".into());
    new.supersedes = Some(standing);
    proposal.decisions.insert(new.id, new);
    proposal
}

#[test]
fn an_owner_cannot_release_a_successor_stranded_behind_its_excluded_work() {
    let (mut plan, _) = excluded_in_flight();
    let audit = id(&plan, "SUP-A-AUDIT");
    let mut proposal = plan.clone();
    proposal
        .work_items
        .get_mut(&audit)
        .expect("audit")
        .contract
        .join = JoinPolicy::ActiveBranches { allow_empty: true };
    match refused_as(&mut plan, proposal.clone()) {
        RelaxedConstraint::Gate { work, gate, .. } => {
            assert_eq!(work, Key::new("SUP-A-AUDIT"));
            assert!(matches!(
                gate,
                UnmetGate::Dependency { .. } | UnmetGate::Applicability { .. }
            ));
        }
        other => panic!("expected a released gate, got {other:?}"),
    }
    apply(&mut plan, reviewer(), proposal).expect("an independent reviewer may");
}

#[test]
fn an_owner_cannot_reselect_its_own_excluded_work() {
    let (mut plan, standing) = excluded_in_flight();
    let proposal = replace(&plan, standing, "A");
    match refused_as(&mut plan, proposal.clone()) {
        RelaxedConstraint::Gate { work, gate, .. } => {
            assert_eq!(work, Key::new("SUP-A-QUOTE"));
            assert!(matches!(
                gate,
                UnmetGate::Applicability {
                    applicability: Applicability::NotSelected { .. }
                }
            ));
        }
        other => panic!("expected a released gate, got {other:?}"),
    }
    apply(&mut plan, reviewer(), proposal).expect("an independent reviewer may");
}

#[test]
fn a_former_holder_cannot_relax_gates_after_handing_work_on() {
    let (mut plan, _) = owned_predecessor();
    let work = id(&plan, "TEST-A");
    run(
        &mut plan,
        reviewer(),
        Command::Handoff {
            work,
            from: owner(),
            to: ActorId::human("successor"),
            reason: "owner reassigned".into(),
        },
        t(2),
    );
    let mut proposal = plan.clone();
    let (a, b) = (id(&plan, "TEST-A"), id(&plan, "TEST-B"));
    proposal
        .dependencies
        .retain(|d| !(d.predecessor == a && d.successor == b));
    let before = plan.clone();
    let error = apply(&mut plan, owner(), proposal).expect_err("former holder refused");
    assert!(
        matches!(error, EngineError::OwnGateRelaxed { .. }),
        "{error}"
    );
    assert_eq!(plan, before);
}

#[test]
fn an_owner_cannot_relax_the_result_it_hands_on_through_a_milestone() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let mut proposal = plan.clone();
    let mut milestone = proposal.work_items[&id(&plan, "TEST-M1")].clone();
    milestone.id = WorkItemId::new();
    milestone.key = Key::new("TEST-M2");
    let (a, c, m2) = (id(&plan, "TEST-A"), id(&plan, "TEST-C"), milestone.id);
    proposal.work_items.insert(m2, milestone);
    proposal
        .dependencies
        .push(Dependency::new(a, m2, DependencyKind::FinishStart, 0.0));
    proposal
        .dependencies
        .push(Dependency::new(m2, c, DependencyKind::FinishStart, 5.0));
    apply(&mut plan, reviewer(), proposal).expect("reviewer adds the milestone");
    run(&mut plan, owner(), Command::Claim { work: a }, t(1));
    let removed = {
        let mut proposal = plan.clone();
        proposal
            .dependencies
            .retain(|d| !(d.predecessor == m2 && d.successor == c));
        proposal
    };
    let lowered = {
        let mut proposal = plan.clone();
        for edge in &mut proposal.dependencies {
            if edge.predecessor == m2 && edge.successor == c {
                edge.lag_hours = 0.0;
            }
        }
        proposal
    };
    for (label, proposal) in [("removed", removed), ("lowered", lowered)] {
        let before = plan.clone();
        let error = apply(&mut plan, owner(), proposal.clone()).expect_err(label);
        assert!(
            matches!(error, EngineError::OwnGateRelaxed { .. }),
            "{label}: {error}"
        );
        assert_eq!(plan, before, "{label}");
        let mut independent = plan.clone();
        apply(&mut independent, reviewer(), proposal).expect("an independent reviewer may apply");
    }
}

#[test]
fn other_inputs_into_a_downstream_milestone_are_not_the_owners_result() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let mut proposal = plan.clone();
    let mut milestone = proposal.work_items[&id(&plan, "TEST-M1")].clone();
    milestone.id = WorkItemId::new();
    milestone.key = Key::new("TEST-M2");
    let (a, c, d, m2) = (
        id(&plan, "TEST-A"),
        id(&plan, "TEST-C"),
        id(&plan, "TEST-D"),
        milestone.id,
    );
    proposal.work_items.insert(m2, milestone);
    for (from, to) in [(a, m2), (d, m2), (m2, c)] {
        proposal
            .dependencies
            .push(Dependency::new(from, to, DependencyKind::FinishStart, 0.0));
    }
    apply(&mut plan, reviewer(), proposal).expect("reviewer adds the milestone");
    run(&mut plan, owner(), Command::Claim { work: a }, t(1));
    let mut proposal = plan.clone();
    proposal
        .dependencies
        .retain(|e| !(e.predecessor == d && e.successor == m2));
    apply(&mut plan, owner(), proposal).expect("another task's input is not the owner's result");
}
