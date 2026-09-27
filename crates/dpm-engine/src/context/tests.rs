use super::*;
use crate::{Command, NextWorkQuery, apply_command, is_ready, next_work};
use chrono::Utc;
use dpm_model::{ActorId, ArtifactId, ArtifactKind, DecisionStatus, WorkItemId};
use std::collections::BTreeMap;

#[test]
fn contextual_decisions_supply_sources_without_gating_or_leaking_to_other_work() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let task = plan.find_work_by_key("TEST-A").expect("task").id;
    let other = plan.find_work_by_key("TEST-B").expect("other").id;
    let source = Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::File,
        uri: "discussion:design-choice".into(),
        label: "Design source, not completion evidence".into(),
        metadata: BTreeMap::new(),
        created_by: ActorId::human("planner"),
        created_at: Utc::now(),
    };
    plan.artifacts.insert(source.id, source.clone());
    let decision = plan.decisions.values_mut().next().expect("decision");
    decision.blocks.clear();
    decision.related_work.insert(task);
    decision.rationale = Some("Keep work independent of checkout paths.".into());
    decision.artifact_ids.insert(source.id);
    let id = decision.id;
    plan.validate().expect("valid associations");

    let before = plan.clone();
    let context = execution_context(&plan, &plan.work_items[&task]);
    assert_eq!(context.decisions, vec![plan.decisions[&id].clone()]);
    assert!(context.artifacts.contains(&source));
    assert_eq!(context.decisions[0].status, DecisionStatus::Open);
    assert!(
        execution_context(&plan, &plan.work_items[&other])
            .decisions
            .is_empty()
    );
    assert!(
        !execution_context(&plan, &plan.work_items[&other])
            .artifacts
            .contains(&source)
    );
    assert!(is_ready(&plan, &plan.work_items[&task], chrono::Utc::now()));
    assert!(
        next_work(&plan, &NextWorkQuery::default(), chrono::Utc::now())
            .expect("next")
            .iter()
            .any(|candidate| candidate.work.id == task)
    );
    assert_eq!(plan, before);
    assert!(
        !plan.work_items[&task]
            .execution
            .artifact_ids
            .contains(&source.id)
    );
    apply_command(
        &mut plan,
        ActorId::agent("worker"),
        Command::Claim { work: task },
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("contextual question does not prevent a claim");

    let mut gated = before;
    gated
        .decisions
        .get_mut(&id)
        .expect("decision")
        .blocks
        .insert(task);
    assert!(!is_ready(
        &gated,
        &gated.work_items[&task],
        chrono::Utc::now()
    ));
}

#[test]
fn context_associations_apply_to_descendants_without_affecting_readiness() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let task = plan.find_work_by_key("TEST-A").expect("task").id;
    let mut parent = plan.work_items[&task].clone();
    parent.id = WorkItemId::new();
    parent.key = dpm_model::Key::new("PACKAGE");
    parent.kind = dpm_model::WorkKind::WorkPackage;
    parent.schedule.estimate = None;
    let parent_id = parent.id;
    plan.work_items.insert(parent_id, parent);
    plan.work_items.get_mut(&task).expect("task").parent = Some(parent_id);
    let decision = plan.decisions.values_mut().next().expect("decision");
    decision.blocks.clear();
    decision.related_work.insert(parent_id);
    let id = decision.id;
    plan.validate().expect("valid hierarchy");
    assert_eq!(
        execution_context(&plan, &plan.work_items[&task]).decisions[0].id,
        id
    );
    assert!(is_ready(&plan, &plan.work_items[&task], chrono::Utc::now()));
}
