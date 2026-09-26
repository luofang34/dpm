use crate::{Command, EngineError, Operation, apply_command, propose_change};
use chrono::Utc;
use dpm_model::{
    ActorId, Dependency, DependencyId, DependencyKind, DependencyPolicy, Plan, WorkItemId,
};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn key(plan: &Plan, key: &str) -> WorkItemId {
    plan.find_work_by_key(key).expect("work").id
}

fn run(plan: &mut Plan, actor: ActorId, command: Command) -> Result<Operation, EngineError> {
    apply_command(plan, actor, command, Utc::now())
}

fn apply(plan: &mut Plan, proposal: Plan) -> Result<Operation, EngineError> {
    run(
        plan,
        ActorId::human("reviewer"),
        Command::ApplyChange {
            plan: Box::new(proposal),
            reason: "reviewed dependency policy".into(),
        },
    )
}

fn edge_mut(plan: &mut Plan, id: DependencyId) -> &mut Dependency {
    plan.dependencies
        .iter_mut()
        .find(|d| d.id == id)
        .expect("edge")
}

type EdgeEdit = fn(&mut Dependency, WorkItemId);

/// Adds soft SS and FF relations between TEST-A and TEST-C through a reviewed change.
fn with_parallel_relations() -> (Plan, DependencyId, DependencyId) {
    let mut plan = fixture();
    let (a, c) = (key(&plan, "TEST-A"), key(&plan, "TEST-C"));
    let mut proposal = plan.clone();
    let mut ids = Vec::new();
    for (kind, lag) in [
        (DependencyKind::StartStart, 2.0),
        (DependencyKind::FinishFinish, 4.0),
    ] {
        let mut edge = Dependency::new(a, c, kind, lag);
        edge.id = DependencyId::new();
        edge.policy = DependencyPolicy::Soft;
        edge.rationale = Some("C overlaps A once its interface is fixed".into());
        ids.push(edge.id);
        proposal.dependencies.push(edge);
    }
    let preview = propose_change(&plan, &proposal).expect("preview");
    let added: Vec<_> = preview
        .changes
        .iter()
        .filter(|c| c.collection == "dependencies" && c.before.is_null())
        .map(|c| c.id.clone().expect("edge id"))
        .collect();
    assert_eq!(added.len(), 2);
    assert!(ids.iter().all(|id| added.contains(&id.to_string())));
    apply(&mut plan, proposal).expect("apply");
    (plan, ids[0], ids[1])
}

#[test]
fn start_start_and_finish_finish_edges_are_edited_independently() {
    let (mut plan, ss, ff) = with_parallel_relations();
    let ss_before = plan.find_dependency(ss).expect("ss").clone();
    let mut proposal = plan.clone();
    edge_mut(&mut proposal, ff).lag_hours = 6.0;
    let preview = propose_change(&plan, &proposal).expect("preview");
    assert_eq!(preview.changes.len(), 1);
    let change = &preview.changes[0];
    assert_eq!(
        (change.collection.as_str(), change.id.clone()),
        ("dependencies", Some(ff.to_string()))
    );
    assert_eq!(change.fields, ["lag_hours"]);
    apply(&mut plan, proposal).expect("apply");
    assert_eq!(plan.find_dependency(ff).expect("ff").lag_hours, 6.0);
    assert_eq!(plan.find_dependency(ss), Some(&ss_before));
    run(
        &mut plan,
        ActorId::human("lead"),
        Command::WaiveDependency {
            dependency: ss,
            reason: "overlap no longer needed".into(),
        },
    )
    .expect("waive SS only");
    assert!(plan.find_dependency(ss).expect("ss").is_waived());
    assert!(!plan.find_dependency(ff).expect("ff").is_waived());
}

#[test]
fn policy_changes_are_reviewed_plan_changes_and_agents_cannot_approve_them() {
    let mut plan = fixture();
    let edge = plan.dependencies[0].id;
    let mut proposal = plan.clone();
    edge_mut(&mut proposal, edge).policy = DependencyPolicy::Soft;
    edge_mut(&mut proposal, edge).rationale = Some("prototype may unblock the successor".into());
    let preview = propose_change(&plan, &proposal).expect("preview");
    assert_eq!(preview.changes.len(), 1);
    assert_eq!(preview.changes[0].fields, ["policy", "rationale"]);
    let before = plan.clone();
    let refused = run(
        &mut plan,
        ActorId::agent("planner"),
        Command::ApplyChange {
            plan: Box::new(proposal.clone()),
            reason: "self-approve".into(),
        },
    );
    assert!(matches!(refused, Err(EngineError::ActorNotAllowed { .. })));
    assert_eq!(plan, before);
    apply(&mut plan, proposal).expect("reviewed policy change");
    assert_eq!(
        plan.find_dependency(edge).expect("edge").policy,
        DependencyPolicy::Soft
    );
}

#[test]
fn proposals_cannot_record_or_remove_waivers_or_change_started_prerequisites() {
    let (mut plan, ss, _) = with_parallel_relations();
    let mut proposal = plan.clone();
    edge_mut(&mut proposal, ss).waiver = Some(dpm_model::DependencyWaiver {
        actor: ActorId::human("reviewer"),
        at: Utc::now(),
        reason: "bypass the command".into(),
    });
    let before = plan.clone();
    assert!(apply(&mut plan, proposal).is_err());
    assert_eq!(plan, before);
    run(
        &mut plan,
        ActorId::human("lead"),
        Command::WaiveDependency {
            dependency: ss,
            reason: "overlap accepted".into(),
        },
    )
    .expect("waive");
    let mut restored = plan.clone();
    edge_mut(&mut restored, ss).waiver = None;
    let before = plan.clone();
    assert!(apply(&mut plan, restored).is_err());
    assert_eq!(plan, before);
    // A waived edge must be restored before review can harden it.
    let mut hardened = plan.clone();
    edge_mut(&mut hardened, ss).policy = DependencyPolicy::Hard;
    let error = apply(&mut plan, hardened).expect_err("waived hard edge");
    assert!(
        error
            .to_string()
            .contains("hard constraints cannot be waived"),
        "{error}"
    );
    assert_eq!(plan, before);
    let mut unrelated = plan.clone();
    unrelated.workspace.name = "renamed".into();
    apply(&mut plan, unrelated).expect("carrying an unchanged waiver is allowed");

    let (a, b) = (key(&plan, "TEST-A"), key(&plan, "TEST-B"));
    let gate = plan.find_decision_by_key("TEST-GATE").expect("gate").id;
    for (actor, command) in [
        (ActorId::agent("w"), Command::Claim { work: a }),
        (
            ActorId::agent("w"),
            Command::Submit {
                work: a,
                note: None,
            },
        ),
        (
            ActorId::human("r"),
            Command::Verify {
                work: a,
                note: None,
            },
        ),
        (
            ActorId::human("r"),
            Command::Decide {
                decision: gate,
                outcome: "go".into(),
            },
        ),
        (ActorId::agent("w"), Command::Claim { work: b }),
    ] {
        run(&mut plan, actor, command).expect("execution step");
    }
    let edge = plan
        .dependencies
        .iter()
        .find(|d| d.successor == b)
        .expect("edge into B")
        .id;
    let mut proposal = plan.clone();
    edge_mut(&mut proposal, edge).policy = DependencyPolicy::Soft;
    let before = plan.clone();
    let error = apply(&mut plan, proposal).expect_err("protected");
    assert!(error.to_string().contains("prerequisite basis"), "{error}");
    assert_eq!(plan, before);
}

#[test]
fn duplicate_identities_missing_endpoints_cycles_and_stale_revisions_fail_atomically() {
    let (mut plan, ss, _) = with_parallel_relations();
    let (a, c) = (key(&plan, "TEST-A"), key(&plan, "TEST-C"));
    let before = plan.clone();
    let cases: Vec<fn(&mut Plan, WorkItemId, WorkItemId, DependencyId)> = vec![
        |p, a, c, ss| {
            let mut copy = Dependency::new(a, c, DependencyKind::StartFinish, 0.0);
            copy.id = ss;
            p.dependencies.push(copy);
        },
        |p, a, c, _| {
            p.dependencies
                .push(Dependency::new(a, c, DependencyKind::StartStart, 9.0))
        },
        |p, a, _, _| {
            p.dependencies.push(Dependency::new(
                a,
                WorkItemId::new(),
                DependencyKind::FinishStart,
                0.0,
            ));
        },
        |p, a, c, _| {
            p.dependencies
                .push(Dependency::new(c, a, DependencyKind::StartStart, 0.0))
        },
        |p, _, _, ss| {
            edge_mut(p, ss).lag_hours = 1.0;
            p.revision = p.revision.wrapping_sub(1);
        },
    ];
    let expected = [
        "duplicate dependency identity",
        "duplicate SS relation",
        "missing work",
        "contains a cycle",
        "revision conflict",
    ];
    for (edit, fragment) in cases.into_iter().zip(expected) {
        let mut proposal = plan.clone();
        edit(&mut proposal, a, c, ss);
        let preview = propose_change(&plan, &proposal).expect_err("invalid proposal");
        assert!(preview.to_string().contains(fragment), "{preview}");
        let applied = apply(&mut plan, proposal).expect_err("invalid apply");
        assert!(applied.to_string().contains(fragment), "{applied}");
        assert_eq!(plan, before);
    }
}

#[test]
fn a_waiver_never_moves_with_an_edited_or_removed_edge() {
    let (mut plan, ss, _) = with_parallel_relations();
    run(
        &mut plan,
        ActorId::human("lead"),
        Command::WaiveDependency {
            dependency: ss,
            reason: "overlap accepted".into(),
        },
    )
    .expect("waive");
    let other = key(&plan, "TEST-D");
    let edits: [(&str, EdgeEdit); 4] = [
        ("retarget", |e, w| e.successor = w),
        ("re-source", |e, w| e.predecessor = w),
        ("re-kind", |e, _| e.kind = DependencyKind::StartFinish),
        ("lag", |e, _| e.lag_hours = 40.0),
    ];
    for (name, edit) in edits {
        let mut proposal = plan.clone();
        edit(edge_mut(&mut proposal, ss), other);
        let before = plan.clone();
        let error = apply(&mut plan, proposal).expect_err(name);
        assert!(
            error.to_string().contains("restore a waived dependency"),
            "{name}: {error}"
        );
        assert_eq!(plan, before, "{name}");
    }
    let mut removed = plan.clone();
    removed.dependencies.retain(|e| e.id != ss);
    let before = plan.clone();
    assert!(apply(&mut plan, removed).is_err());
    assert_eq!(plan, before);
    run(
        &mut plan,
        ActorId::human("lead"),
        Command::RestoreDependency {
            dependency: ss,
            reason: "edit the constraint instead".into(),
        },
    )
    .expect("restore");
    let mut retargeted = plan.clone();
    edge_mut(&mut retargeted, ss).lag_hours = 40.0;
    apply(&mut plan, retargeted).expect("unwaived edges are edited through review");
}
