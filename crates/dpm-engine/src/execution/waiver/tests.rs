use crate::{
    Command, EngineError, NextWorkQuery, UnmetGate, apply_command, gate_report, next_work,
    progress, status,
};
use chrono::Utc;
use dpm_model::{
    ActorId, DependencyId, DependencyKind, DependencyPolicy, Plan, WorkItemId, WorkLink,
    WorkLinkKind,
};
use dpm_schedule::{deterministic, deterministic_remaining, simulate_remaining};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn key(plan: &Plan, key: &str) -> WorkItemId {
    plan.find_work_by_key(key).expect("work").id
}

/// TEST-A -> TEST-B made soft, with TEST-B's decision gate resolved so only the edge gates it.
fn soft_edge_plan() -> (Plan, DependencyId, WorkItemId) {
    let mut plan = fixture();
    let (a, b) = (key(&plan, "TEST-A"), key(&plan, "TEST-B"));
    let gate = plan.find_decision_by_key("TEST-GATE").expect("gate").id;
    apply_command(
        &mut plan,
        ActorId::human("lead"),
        Command::Decide {
            decision: gate,
            outcome: "proceed".into(),
        },
        Utc::now(),
    )
    .expect("decide");
    let edge = plan
        .dependencies
        .iter_mut()
        .find(|d| d.predecessor == a && d.successor == b)
        .expect("edge");
    edge.policy = DependencyPolicy::Soft;
    edge.rationale = Some("B may start from a prototype of A".into());
    let id = edge.id;
    (plan, id, b)
}

fn json(value: &impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(value).expect("serialize")
}

fn run(plan: &mut Plan, actor: ActorId, command: Command) -> Result<(), EngineError> {
    apply_command(plan, actor, command, Utc::now()).map(|_| ())
}

fn waive(dependency: DependencyId, reason: &str) -> Command {
    Command::WaiveDependency {
        dependency,
        reason: reason.into(),
    }
}

fn restore(dependency: DependencyId, reason: &str) -> Command {
    Command::RestoreDependency {
        dependency,
        reason: reason.into(),
    }
}

#[test]
fn waiver_and_restoration_change_readiness_and_remaining_cpm_consistently() {
    let (mut plan, edge, b) = soft_edge_plan();
    let gates_before =
        gate_report(&plan, b, crate::Transition::Claim, chrono::Utc::now()).expect("gates");
    let remaining_before =
        json(&deterministic_remaining(&plan, chrono::Utc::now()).expect("remaining"));
    let baseline_before = json(&deterministic(&plan).expect("baseline"));
    assert!(!gates_before.ready);
    assert!(gates_before.unmet.iter().any(|g| matches!(g,
        UnmetGate::Dependency { dependency, policy: DependencyPolicy::Soft, .. } if *dependency == edge)));
    assert!(
        remaining_before["activities"][b.to_string()]["earliest_start_hours"].as_f64() > Some(0.0)
    );

    let at = Utc::now();
    let operation = apply_command(
        &mut plan,
        ActorId::human("lead"),
        waive(edge, "prototype of A is sufficient"),
        at,
    )
    .expect("waive");
    assert!(matches!(operation.command, Command::WaiveDependency { .. }));
    let record = plan
        .find_dependency(edge)
        .and_then(|d| d.waiver.clone())
        .expect("waiver");
    assert_eq!(
        (record.actor, record.at, record.reason.as_str()),
        (ActorId::human("lead"), at, "prototype of A is sufficient")
    );
    let waived =
        gate_report(&plan, b, crate::Transition::Claim, chrono::Utc::now()).expect("gates");
    assert!(waived.ready, "{waived:?}");
    let verify =
        gate_report(&plan, b, crate::Transition::Verify, chrono::Utc::now()).expect("gates");
    assert!(
        !verify
            .unmet
            .iter()
            .any(|g| matches!(g, crate::UnmetGate::Dependency { .. }))
    );
    let remaining = deterministic_remaining(&plan, chrono::Utc::now()).expect("remaining");
    assert_eq!(remaining.activities[&b].earliest_start_hours, 0.0);
    assert!(
        Some(remaining.project_finish_hours) <= remaining_before["project_finish_hours"].as_f64()
    );
    assert_eq!(
        json(&deterministic(&plan).expect("baseline")),
        baseline_before
    );
    let simulated =
        simulate_remaining(&plan, Default::default(), chrono::Utc::now()).expect("simulation");
    assert!(simulated.criticality.contains_key(&b));
    let ready: Vec<_> = next_work(&plan, &NextWorkQuery::default(), chrono::Utc::now())
        .expect("next")
        .into_iter()
        .map(|c| c.work.id)
        .collect();
    assert!(ready.contains(&b));

    run(
        &mut plan,
        ActorId::service("release-bot"),
        restore(edge, "prototype rejected"),
    )
    .expect("restore");
    assert!(plan.find_dependency(edge).expect("edge").waiver.is_none());
    assert_eq!(
        gate_report(&plan, b, crate::Transition::Claim, chrono::Utc::now()).expect("gates"),
        gates_before
    );
    assert_eq!(
        json(&deterministic_remaining(&plan, chrono::Utc::now()).expect("remaining")),
        remaining_before
    );
}

#[test]
fn unwaived_soft_edges_still_gate_and_invalid_waivers_fail_atomically() {
    let (mut plan, edge, b) = soft_edge_plan();
    assert!(
        !gate_report(&plan, b, crate::Transition::Claim, chrono::Utc::now())
            .expect("gates")
            .ready
    );
    let hard = plan
        .dependencies
        .iter()
        .find(|d| d.policy == DependencyPolicy::Hard)
        .expect("hard edge")
        .id;
    let cases = [
        (ActorId::agent("coder"), waive(edge, "skip it")),
        (ActorId::human("lead"), waive(edge, "  ")),
        (ActorId::human("lead"), waive(hard, "skip it")),
        (
            ActorId::human("lead"),
            waive(DependencyId::new(), "skip it"),
        ),
        (ActorId::human("lead"), restore(edge, "never waived")),
    ];
    for (actor, command) in cases {
        let before = plan.clone();
        let error = run(&mut plan, actor, command).expect_err("rejected");
        assert_eq!(plan, before, "{error}");
    }
    run(&mut plan, ActorId::human("lead"), waive(edge, "prototype")).expect("waive");
    let before = plan.clone();
    for (actor, command) in [
        (ActorId::human("lead"), waive(edge, "again")),
        (ActorId::agent("coder"), restore(edge, "agent restore")),
        (ActorId::human("lead"), restore(edge, "")),
    ] {
        assert!(run(&mut plan, actor, command).is_err());
        assert_eq!(plan, before);
    }
}

#[test]
fn a_milestone_whose_only_prerequisite_is_waived_stays_unreached() {
    let mut plan = fixture();
    let milestone = key(&plan, "TEST-M1");
    let edge = plan
        .dependencies
        .iter_mut()
        .find(|d| d.successor == milestone)
        .expect("edge");
    edge.policy = DependencyPolicy::Soft;
    let id = edge.id;
    run(&mut plan, ActorId::human("lead"), waive(id, "scope cut")).expect("waive");
    assert!(!crate::completion(&plan, chrono::Utc::now()).contains(&milestone));
}

#[test]
fn non_gating_links_leave_readiness_schedule_progress_and_ranking_unchanged() {
    let (plan, _, _) = soft_edge_plan();
    let observe = |plan: &Plan| {
        serde_json::json!({
            "status": status(plan, true, chrono::Utc::now()).expect("status"),
            "next": next_work(plan, &NextWorkQuery::default(), chrono::Utc::now()).expect("next"),
            "remaining": deterministic_remaining(plan, chrono::Utc::now()).expect("remaining"),
            "baseline": deterministic(plan).expect("baseline"),
            "progress": progress(plan, chrono::Utc::now()).expect("progress"),
            "gates": plan.work_items.keys().map(|id| gate_report(plan, *id, crate::Transition::Claim, chrono::Utc::now()).expect("gates")).collect::<Vec<_>>(),
        })
    };
    let before = observe(&plan);
    let mut linked = plan.clone();
    let (a, c, f) = (
        key(&plan, "TEST-A"),
        key(&plan, "TEST-C"),
        key(&plan, "TEST-F"),
    );
    for (kind, source, target) in [
        (WorkLinkKind::RelatesTo, a, f),
        (WorkLinkKind::Duplicates, f, c),
        (WorkLinkKind::DerivedFrom, c, a),
        (WorkLinkKind::Supersedes, f, a),
    ] {
        linked.links.push(WorkLink {
            kind,
            source,
            target,
            note: Some("context only".into()),
        });
    }
    linked.validate().expect("links");
    assert_eq!(observe(&linked), before);
    let explained = crate::explain_work(&linked, a, chrono::Utc::now()).expect("explain");
    assert_eq!(explained.context.links.len(), 3);
    assert_eq!(
        explained.gates,
        crate::explain_work(&plan, a, chrono::Utc::now())
            .expect("explain")
            .gates
    );
}

#[test]
fn relation_kinds_between_one_pair_are_addressed_by_distinct_identities() {
    let (mut plan, edge, b) = soft_edge_plan();
    let a = key(&plan, "TEST-A");
    let mut ss = dpm_model::Dependency::new(a, b, DependencyKind::StartStart, 1.0);
    ss.policy = DependencyPolicy::Soft;
    let ss_id = ss.id;
    plan.dependencies.push(ss);
    run(&mut plan, ActorId::human("lead"), waive(edge, "FS relaxed")).expect("waive");
    assert!(plan.find_dependency(edge).expect("fs").is_waived());
    assert!(!plan.find_dependency(ss_id).expect("ss").is_waived());
    let gates = gate_report(&plan, b, crate::Transition::Claim, chrono::Utc::now()).expect("gates");
    assert!(!gates.ready);
    assert!(gates.unmet.iter().any(|g| matches!(g,
        UnmetGate::Dependency { dependency, relation: DependencyKind::StartStart, .. } if *dependency == ss_id)));
}

fn refused_to(plan: &mut Plan, actor: &ActorId, command: Command) {
    let before = plan.clone();
    let error = run(plan, actor.clone(), command).expect_err("refused");
    assert!(
        matches!(error, EngineError::ActorNotAllowed { .. }),
        "{actor}: {error:?}"
    );
    assert_eq!(*plan, before, "a refused command changes nothing");
}

#[test]
fn owners_of_either_endpoint_can_neither_waive_nor_restore_the_edge() {
    let (mut plan, edge, b) = soft_edge_plan();
    let a = key(&plan, "TEST-A");
    let (alice, bob, lead) = (
        ActorId::human("alice"),
        ActorId::human("bob"),
        ActorId::human("lead"),
    );
    run(&mut plan, alice.clone(), Command::Claim { work: a }).expect("claim A");
    refused_to(&mut plan, &alice, waive(edge, "my result is good enough"));
    run(
        &mut plan,
        lead.clone(),
        waive(edge, "prototype of A is sufficient"),
    )
    .expect("waive");
    run(&mut plan, bob.clone(), Command::Claim { work: b }).expect("claim B");
    for owner in [&alice, &bob] {
        refused_to(&mut plan, owner, restore(edge, "my own call"));
    }
    run(
        &mut plan,
        ActorId::service("release-bot"),
        restore(edge, "prototype rejected"),
    )
    .expect("independent restoration");
    refused_to(&mut plan, &bob, waive(edge, "skip my own prerequisite"));
    run(&mut plan, lead, waive(edge, "independent call")).expect("independent waiver");
}

#[test]
fn a_successor_owner_cannot_waive_away_its_own_invalidated_provisional_basis() {
    let (mut plan, edge, b) = soft_edge_plan();
    let a = key(&plan, "TEST-A");
    plan.dependencies
        .iter_mut()
        .find(|d| d.id == edge)
        .expect("edge")
        .start_basis = dpm_model::StartBasis::Provisional;
    let (author, owner, reviewer) = (
        ActorId::agent("author"),
        ActorId::human("bob"),
        ActorId::human("reviewer"),
    );
    let reject = Command::Reject {
        work: a,
        reason: "acceptance check fails".into(),
    };
    for (actor, command) in [
        (&author, Command::Claim { work: a }),
        (&author, Command::Start { work: a }),
        (
            &author,
            Command::Submit {
                work: a,
                note: None,
            },
        ),
        (&owner, Command::Claim { work: b }),
        (&owner, Command::Start { work: b }),
        (&reviewer, reject),
    ] {
        run(&mut plan, actor.clone(), command).expect("setup");
    }
    let invalidated = |plan: &Plan| {
        gate_report(plan, b, crate::Transition::Submit, Utc::now())
            .expect("gates")
            .unmet
            .iter()
            .any(|g| matches!(g, UnmetGate::BasisInvalidated { .. }))
    };
    assert!(invalidated(&plan));
    refused_to(&mut plan, &owner, waive(edge, "my work is fine"));
    assert!(invalidated(&plan));
    run(&mut plan, reviewer, waive(edge, "B no longer uses A")).expect("independent waiver");
    assert!(!invalidated(&plan));
}
