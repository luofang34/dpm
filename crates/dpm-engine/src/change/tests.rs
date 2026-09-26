use super::*;
use crate::{Command, apply_command};
use chrono::Utc;
use dpm_model::{ActorId, DecisionStatus, Key, WorkItemId, WorkStatus};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn apply(plan: &mut Plan, proposed: Plan) -> Result<crate::Operation, EngineError> {
    apply_command(
        plan,
        ActorId::human("planner"),
        Command::ApplyChange {
            plan: Box::new(proposed),
            reason: "clarify remaining work".into(),
        },
        Utc::now(),
    )
}

#[test]
fn proposals_show_semantic_changes_and_apply_as_one_reviewed_operation() {
    let mut plan = fixture();
    let before = plan.clone();
    let mut proposal = plan.clone();
    proposal.find_work_by_key_mut("TEST-A").expect("task").title = "Clarified contract".into();
    let mut added = proposal.find_work_by_key("TEST-A").expect("task").clone();
    added.id = WorkItemId::new();
    added.key = Key::new("NEW");
    added.status = WorkStatus::Proposed;
    added.artifact_ids.clear();
    proposal.work_items.insert(added.id, added.clone());
    let preview = propose_change(&plan, &proposal).expect("proposal");
    assert_eq!(plan, before);
    assert_eq!(preview.base_revision, 0);
    assert_eq!(preview.changes.len(), 2);
    assert!(preview.changes.iter().any(|c| c.fields == ["title"]));
    assert!(
        preview
            .changes
            .iter()
            .any(|c| c.before.is_null() && c.after["key"] == "NEW")
    );
    let operation = apply(&mut plan, proposal).expect("apply");
    assert_eq!(operation.base_revision, 0);
    assert_eq!(plan.revision, 1);
    assert!(!crate::is_ready(&plan, &plan.work_items[&added.id]));
    assert_eq!(
        plan.find_work_by_key("TEST-A").expect("work").title,
        "Clarified contract"
    );
}

#[test]
fn agent_approval_empty_reason_stale_revision_and_noop_fail_atomically() {
    let mut plan = fixture();
    let before = plan.clone();
    let mut proposal = plan.clone();
    proposal.workspace.name = "new name".into();
    for (actor, reason) in [
        (ActorId::agent("drafter"), "change"),
        (ActorId::human("lead"), " "),
    ] {
        assert!(
            apply_command(
                &mut plan,
                actor,
                Command::ApplyChange {
                    plan: Box::new(proposal.clone()),
                    reason: reason.into()
                },
                Utc::now()
            )
            .is_err()
        );
        assert_eq!(plan, before);
    }
    assert!(apply(&mut plan, before.clone()).is_err());
    proposal.revision = 99;
    assert!(matches!(
        apply(&mut plan, proposal),
        Err(EngineError::RevisionConflict { .. })
    ));
    assert_eq!(plan, before);
}

#[test]
fn invalid_graphs_and_fabricated_execution_fail_without_changes() {
    let mut plan = fixture();
    let before = plan.clone();
    for mutation in 0..5 {
        let mut proposal = plan.clone();
        match mutation {
            0 => proposal.dependencies.push(dpm_model::Dependency {
                predecessor: proposal.dependencies[0].successor,
                successor: proposal.dependencies[0].predecessor,
                kind: dpm_model::DependencyKind::FinishStart,
                lag_hours: 0.0,
            }),
            1 => {
                proposal.find_work_by_key_mut("TEST-A").expect("task").owner =
                    Some(ActorId::agent("fake"));
            }
            2 => {
                let work = proposal.find_work_by_key_mut("TEST-A").expect("task");
                work.status = WorkStatus::Verified;
                work.owner = Some(ActorId::agent("fake"));
            }
            3 => {
                let decision = proposal.decisions.values_mut().next().expect("gate");
                decision.status = DecisionStatus::Decided;
                decision.outcome = Some("bypass".into());
            }
            _ => {
                proposal.workspace.id = dpm_model::WorkspaceId::new();
            }
        }
        assert!(apply(&mut plan, proposal).is_err());
        assert_eq!(plan, before);
    }
}

#[test]
fn execution_locks_its_contract_context_and_dependencies_but_not_future_work() {
    let mut plan = fixture();
    let work = plan.find_work_by_key("TEST-A").expect("work").id;
    apply_command(
        &mut plan,
        ActorId::agent("worker"),
        Command::Claim { work },
        Utc::now(),
    )
    .expect("claim");
    let before = plan.clone();
    for mutation in 0..4 {
        let mut proposal = plan.clone();
        match mutation {
            0 => proposal
                .work_items
                .get_mut(&work)
                .expect("work")
                .acceptance
                .clear(),
            1 => {
                proposal
                    .requirements
                    .values_mut()
                    .next()
                    .expect("requirement")
                    .statement = "lowered bar".into();
            }
            2 => {
                proposal
                    .resources
                    .values_mut()
                    .next()
                    .expect("resource")
                    .label = "other repository".into();
            }
            _ => {
                let mut gate = proposal
                    .decisions
                    .values()
                    .next()
                    .expect("decision")
                    .clone();
                gate.id = dpm_model::DecisionId::new();
                gate.key = Key::new("NEW-GATE");
                gate.blocks.insert(work);
                proposal.decisions.insert(gate.id, gate);
            }
        }
        assert!(apply(&mut plan, proposal).is_err());
        assert_eq!(plan, before);
    }
    let mut proposal = plan.clone();
    proposal.find_work_by_key_mut("TEST-F").expect("work").title = "Future integration".into();
    apply(&mut plan, proposal).expect("future work remains editable");
}

#[test]
fn deletion_is_explicit_in_diff_and_order_only_changes_are_not_semantic() {
    let plan = fixture();
    let mut proposal = plan.clone();
    proposal.dependencies.reverse();
    assert!(
        propose_change(&plan, &proposal)
            .expect("order independent")
            .changes
            .is_empty()
    );
    let id = proposal.find_work_by_key("TEST-F").expect("task").id;
    proposal.work_items.remove(&id);
    proposal
        .dependencies
        .retain(|d| d.predecessor != id && d.successor != id);
    let preview = propose_change(&plan, &proposal).expect("explicit removal");
    assert!(
        preview
            .changes
            .iter()
            .any(|c| c.after.is_null() && c.before["key"] == "TEST-F")
    );
}
