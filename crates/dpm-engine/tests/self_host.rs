//! Prepared self-host contracts are inspectable but are not executed by this suite.
#![allow(clippy::expect_used)]
use dpm_engine::{NextWorkQuery, explain_work, next_work, status};
use dpm_model::{DecisionStatus, Plan, WorkItemId, WorkKind, WorkStatus};
use std::collections::{BTreeSet, VecDeque};

fn fixture() -> Plan {
    serde_json::from_str(include_str!("../../../examples/self-host/dpm-alpha.json"))
        .expect("self-host plan")
}

#[test]
fn prepared_contracts_have_no_owners_progress_or_implicit_authorization() {
    let plan = fixture();
    let before = plan.clone();
    plan.validate().expect("valid graph");
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/self-host/dpm-alpha.expected.json"
    ))
    .expect("expected contract");
    let summary = status(&plan, false).expect("summary");
    assert_eq!(
        summary.total_work as u64,
        expected["total_work"].as_u64().expect("count")
    );
    assert_eq!(
        (summary.revision, summary.ready, summary.complete),
        (0, 0, 0)
    );
    assert_eq!(summary.progress.percent_complete, 0.0);
    assert!(
        next_work(&plan, &NextWorkQuery::default())
            .expect("next")
            .is_empty()
    );
    assert!(
        plan.decisions
            .values()
            .all(|d| d.status == DecisionStatus::Open && d.outcome.is_none())
    );
    for item in plan.work_items.values() {
        assert_eq!(item.status, WorkStatus::Planned);
        assert!(item.owner.is_none());
        assert_eq!(item.reported_progress_percent, 0);
        if item.is_executable() {
            assert!(!item.objective.trim().is_empty());
            assert!(item.acceptance.len() >= 3);
            assert!(
                !item.capabilities.is_empty()
                    && !item.requirement_ids.is_empty()
                    && !item.artifact_ids.is_empty()
            );
            item.estimate
                .expect("provisional O/M/P")
                .validate()
                .expect("estimate");
            assert!(
                reaches_milestone(&plan, item.id),
                "{} has no acceptance milestone",
                item.key
            );
        } else {
            assert!(item.estimate.is_none());
            assert_eq!(item.expected_duration_hours(), 0.0);
        }
    }
    assert_eq!(plan, before);
}

#[test]
fn current_and_deferred_contracts_resolve_context_and_inherited_gates() {
    let plan = fixture();
    for key in ["MVP-10", "TUI-10", "SERVER-10"] {
        let work = plan.find_work_by_key(key).expect("work");
        let detail = explain_work(&plan, work.id).expect("explain");
        assert!(!detail.ready);
        assert!(!detail.context.requirements.is_empty());
        assert!(!detail.context.artifacts.is_empty());
        assert!(!detail.context.risks.is_empty());
        let gates: BTreeSet<_> = detail
            .context
            .decisions
            .iter()
            .map(|d| d.key.0.as_str())
            .collect();
        assert!(gates.contains("DEC-EXECUTE"));
        if key == "SERVER-10" {
            assert!(gates.contains("DEC-EXPAND"));
        }
        if key == "TUI-10" {
            assert!(gates.contains("DEC-LAYOUT"));
        }
    }
    let first = plan.find_work_by_key("MVP-10").expect("first contract");
    let detail = explain_work(&plan, first.id).expect("explain");
    assert!(detail.why_now.iter().any(|why| why.contains("DEC-EXECUTE")));
}

fn reaches_milestone(plan: &Plan, start: WorkItemId) -> bool {
    let mut queue = VecDeque::from([start]);
    let mut seen = BTreeSet::new();
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        if plan.work_items[&id].kind == WorkKind::Milestone {
            return true;
        }
        queue.extend(
            plan.dependencies
                .iter()
                .filter(|d| d.predecessor == id)
                .map(|d| d.successor),
        );
    }
    false
}

#[test]
fn every_task_exposes_ordered_steps_boundaries_and_checks_without_mutation() {
    let plan = fixture();
    let before = plan.clone();
    for work in plan.work_items.values().filter(|w| w.is_executable()) {
        let contract = work.instructions.as_ref().expect("explicit procedure");
        assert!(contract.steps.len() >= 3, "{}", work.key);
        assert!(!contract.in_scope.is_empty() && !contract.out_of_scope.is_empty());
        assert!(!contract.verification.is_empty());
        let detail = explain_work(&plan, work.id).expect("explain");
        assert_eq!(detail.work.instructions.as_ref(), Some(contract));
        assert_eq!(detail.work.acceptance, work.acceptance);
        assert!(!detail.ready);
    }
    assert_eq!(plan, before);
}
