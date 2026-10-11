use super::*;
use crate::{apply_command, plan_change};
use chrono::Utc;
use dpm_model::{ActorId, Decision, DecisionId, DecisionStatus, Key, OperationId, WorkItemId};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn work(plan: &Plan, key: &str) -> WorkItemId {
    plan.work_items
        .values()
        .find(|w| w.key.to_string() == key)
        .map(|w| w.id)
        .expect("work")
}

fn planner() -> ActorId {
    ActorId::human("planner")
}

/// Apply `edit` as one reviewed plan change, returning the recorded command.
fn change(plan: &mut Plan, reason: &str, edit: impl FnOnce(&mut Plan)) -> Command {
    let mut proposed = plan.clone();
    edit(&mut proposed);
    let command = plan_change(plan, &proposed, reason.to_owned()).expect("change");
    apply_command(plan, planner(), command, Utc::now(), OperationId::new())
        .expect("applies")
        .command
}

/// Plans are compared without the revision, which a revert advances like any change.
fn content(plan: &Plan) -> serde_json::Value {
    let mut plan = plan.clone();
    plan.revision = 0;
    plan.dependencies.sort_by_key(|edge| edge.id);
    serde_json::to_value(plan).expect("json")
}

#[test]
fn a_revert_applied_through_review_restores_the_plan_and_names_the_change_it_undoes() {
    let mut plan = fixture();
    let original = plan.clone();
    let b = work(&plan, "TEST-B");
    let recorded = change(&mut plan, "Rename and widen B", |p| {
        if let Some(item) = p.work_items.get_mut(&b) {
            item.title = "Contract B, renamed".into();
            if let Some(estimate) = item.schedule.estimate.as_mut() {
                estimate.likely_hours += 5.0;
            }
        }
    });
    let reverted = OperationId::new();
    let preview = propose_revert(&plan, reverted, &recorded).expect("the inverse reviews");
    assert!(!preview.changes.is_empty());
    let revision = plan.revision;
    let command = Command::RevertChange {
        operation: reverted,
        changes: preview.changes,
        reason: "Undo".into(),
    };
    let operation = apply_command(
        &mut plan,
        planner(),
        command,
        Utc::now(),
        OperationId::new(),
    )
    .expect("the revert applies");
    assert_eq!(
        content(&plan),
        content(&original),
        "the plan reads as before the change"
    );
    assert_eq!(
        plan.revision,
        revision + 1,
        "a revert is a new operation, not a rewrite"
    );
    assert!(
        matches!(operation.command, Command::RevertChange { operation, .. } if operation == reverted)
    );
}

#[test]
fn a_revert_over_later_changes_is_refused_naming_each_entity_that_changed() {
    let mut plan = fixture();
    let b = work(&plan, "TEST-B");
    let recorded = change(&mut plan, "Rename B", |p| {
        if let Some(item) = p.work_items.get_mut(&b) {
            item.title = "First".into();
        }
    });
    change(&mut plan, "Rename B again", |p| {
        if let Some(item) = p.work_items.get_mut(&b) {
            item.title = "Second".into();
        }
    });
    let revision = plan.revision;
    let refused =
        propose_revert(&plan, OperationId::new(), &recorded).expect_err("a later change conflicts");
    let message = refused.to_string();
    assert!(
        message.contains("TEST-B") && message.contains(&b.to_string()),
        "names the entity: {message}"
    );
    assert_eq!(plan.revision, revision);
}

#[test]
fn a_revert_that_would_rewrite_started_work_is_refused_like_any_change() {
    let mut plan = fixture();
    let a = work(&plan, "TEST-A");
    let recorded = change(&mut plan, "Reword A", |p| {
        if let Some(item) = p.work_items.get_mut(&a) {
            item.contract.objective = "A reworded objective".into();
        }
    });
    let agent = ActorId::agent("pilot");
    for command in [
        Command::Claim { work: a },
        Command::Start {
            work: a,
            occurred_at: None,
        },
    ] {
        apply_command(
            &mut plan,
            agent.clone(),
            command,
            Utc::now(),
            OperationId::new(),
        )
        .expect("lifecycle");
    }
    propose_revert(&plan, OperationId::new(), &recorded)
        .expect_err("started work's contract is protected");
}

#[test]
fn only_a_plan_change_can_be_reverted() {
    let plan = fixture();
    let claim = Command::Claim {
        work: work(&plan, "TEST-A"),
    };
    let refused = propose_revert(&plan, OperationId::new(), &claim)
        .expect_err("a lifecycle event is not a plan change");
    assert!(refused.to_string().contains("only a recorded plan change"));
}

#[test]
fn a_reverted_revert_reapplies_the_original_change() {
    let mut plan = fixture();
    let b = work(&plan, "TEST-B");
    let recorded = change(&mut plan, "Rename B", |p| {
        if let Some(item) = p.work_items.get_mut(&b) {
            item.title = "Renamed".into();
        }
    });
    let renamed = plan.clone();
    let undo = Command::RevertChange {
        operation: OperationId::new(),
        changes: propose_revert(&plan, OperationId::new(), &recorded)
            .expect("undo")
            .changes,
        reason: "Undo".into(),
    };
    let undone = apply_command(&mut plan, planner(), undo, Utc::now(), OperationId::new())
        .expect("undo applies")
        .command;
    let redo =
        propose_revert(&plan, OperationId::new(), &undone).expect("the revert itself reverts");
    let redo = Command::RevertChange {
        operation: OperationId::new(),
        changes: redo.changes,
        reason: "Redo".into(),
    };
    apply_command(&mut plan, planner(), redo, Utc::now(), OperationId::new())
        .expect("redo applies");
    assert_eq!(content(&plan), content(&renamed));
}

/// Applying a replacement stamps when its new decision was resolved, which the recorded change cannot carry.
#[test]
fn a_reverted_replacement_is_judged_by_the_decision_rules_not_as_changed_since() {
    let mut plan = fixture();
    let project = plan.work_items[&work(&plan, "TEST-A")].project;
    let old = DecisionId::new();
    plan.decisions.insert(
        old,
        Decision {
            id: old,
            key: Key::new("TEST-FORMAT"),
            project,
            question: "Which input format?".into(),
            status: DecisionStatus::Decided,
            outcome: Some("JSON".into()),
            resolved_at: None,
            rationale: Some("Existing tooling reads JSON".into()),
            related_work: Default::default(),
            artifact_ids: Default::default(),
            blocks: Default::default(),
            supersedes: None,
            options: Vec::new(),
        },
    );
    plan.validate().expect("valid fixture");
    let recorded = change(&mut plan, "Replace the format choice", |p| {
        let mut new = p.decisions[&old].clone();
        if let Some(previous) = p.decisions.get_mut(&old) {
            previous.status = DecisionStatus::Superseded;
        }
        new.id = DecisionId::new();
        new.key = Key::new("TEST-FORMAT-2");
        new.outcome = Some("TOML".into());
        new.supersedes = Some(old);
        p.decisions.insert(new.id, new);
    });
    assert!(
        plan.decisions
            .values()
            .any(|d| d.key.to_string() == "TEST-FORMAT-2" && d.resolved_at.is_some())
    );
    // Nothing changed after the replacement: the refusal is the decision rule's, never a later change.
    let refused = propose_revert(&plan, OperationId::new(), &recorded)
        .expect_err("a decided choice is not un-decided")
        .to_string();
    assert!(!refused.contains("changed after it"), "{refused}");
    assert!(refused.contains("replacement"), "{refused}");
}

#[test]
fn a_revert_after_a_lifecycle_event_says_so() {
    let mut plan = fixture();
    let a = work(&plan, "TEST-A");
    let recorded = change(&mut plan, "Reword A", |p| {
        if let Some(item) = p.work_items.get_mut(&a) {
            item.contract.objective = "A reworded objective".into();
        }
    });
    apply_command(
        &mut plan,
        ActorId::agent("pilot"),
        Command::Claim { work: a },
        Utc::now(),
        OperationId::new(),
    )
    .expect("claim");
    let message = propose_revert(&plan, OperationId::new(), &recorded)
        .expect_err("claimed since")
        .to_string();
    assert!(
        message.contains("TEST-A") && message.contains("lifecycle event"),
        "{message}"
    );
}
