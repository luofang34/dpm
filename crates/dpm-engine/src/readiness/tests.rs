use super::*;
use crate::{Command, apply_command, explain_work};
use chrono::Utc;
use dpm_model::{ActorId, Key};

fn nested_plan() -> (Plan, WorkItemId, WorkItemId) {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let task = plan.find_work_by_key("TEST-A").expect("task").id;
    plan.work_items.retain(|id, _| *id == task);
    plan.dependencies.clear();
    plan.risks.clear();
    let mut package = plan.work_items[&task].clone();
    package.id = WorkItemId::new();
    package.key = Key::new("PACKAGE");
    package.kind = WorkKind::WorkPackage;
    package.estimate = None;
    let parent = package.id;
    plan.work_items.insert(parent, package);
    plan.work_items.get_mut(&task).expect("task").parent = Some(parent);
    plan.decisions
        .values_mut()
        .for_each(|d| d.blocks = BTreeSet::from([parent]));
    (plan, task, parent)
}

#[test]
fn inherited_decision_gates_block_children_and_explain_the_gate_key() {
    let (mut plan, task, _) = nested_plan();
    assert!(!is_ready(&plan, &plan.work_items[&task]));
    assert!(
        explain_work(&plan, task)
            .expect("explain")
            .why_now
            .iter()
            .any(|line| line.contains("TEST-GATE"))
    );
    let decision = plan.decisions.values().next().expect("decision").id;
    apply_command(
        &mut plan,
        ActorId::human("reviewer"),
        Command::Decide {
            decision,
            outcome: "approved".into(),
        },
        Utc::now(),
    )
    .expect("decide");
    assert!(is_ready(&plan, &plan.work_items[&task]));
}

#[test]
fn nested_packages_complete_from_children_without_persisting_completion() {
    let (mut plan, task, parent) = nested_plan();
    plan.decisions.clear();
    let mut outer = plan.work_items[&parent].clone();
    outer.id = WorkItemId::new();
    outer.key = Key::new("OUTER");
    let outer_id = outer.id;
    plan.work_items.insert(outer_id, outer);
    plan.work_items.get_mut(&parent).expect("package").parent = Some(outer_id);
    for (actor, command) in [
        ("worker", Command::Claim { work: task }),
        (
            "worker",
            Command::Submit {
                work: task,
                note: None,
            },
        ),
        (
            "reviewer",
            Command::Verify {
                work: task,
                note: None,
            },
        ),
    ] {
        apply_command(&mut plan, ActorId::agent(actor), command, Utc::now()).expect("execute");
    }
    assert_eq!(completion(&plan), BTreeSet::from([task, parent, outer_id]));
    assert_eq!(
        show_work(&plan, outer_id).expect("projection").status,
        WorkStatus::Verified
    );
    assert_eq!(plan.work_items[&outer_id].status, WorkStatus::Planned);
}

#[test]
fn empty_aggregates_and_proposed_tasks_are_not_complete_or_ready() {
    let (mut plan, task, parent) = nested_plan();
    plan.decisions.clear();
    plan.work_items.get_mut(&task).expect("task").parent = None;
    plan.work_items.get_mut(&task).expect("task").status = WorkStatus::Proposed;
    assert!(!is_ready(&plan, &plan.work_items[&task]));
    assert!(!completion(&plan).contains(&parent));
    plan.work_items.get_mut(&parent).expect("package").kind = WorkKind::Milestone;
    assert!(!completion(&plan).contains(&parent));
}

#[test]
fn readiness_rejects_foreign_stale_or_invalid_contracts() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let work = plan.find_work_by_key("TEST-A").expect("work").clone();
    assert!(is_ready(&plan, &work));
    let mut foreign = work.clone();
    foreign.id = WorkItemId::new();
    assert!(!is_ready(&plan, &foreign));
    apply_command(
        &mut plan,
        ActorId::agent("owner"),
        Command::Claim { work: work.id },
        Utc::now(),
    )
    .expect("claim");
    assert!(!is_ready(&plan, &work));
    let stored = plan.work_items.get_mut(&work.id).expect("work");
    stored.status = WorkStatus::Planned;
    stored.owner = None;
    stored.acceptance.clear();
    let invalid = stored.clone();
    assert!(!is_ready(&plan, &invalid));
    let before = plan.clone();
    assert!(
        apply_command(
            &mut plan,
            ActorId::agent("owner"),
            Command::Claim { work: work.id },
            Utc::now()
        )
        .is_err()
    );
    assert_eq!(plan, before);
}
