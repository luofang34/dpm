//! Prepared self-host contracts are inspectable but are not executed by this suite.
#![cfg(test)]
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
    let proposed: BTreeSet<_> = expected["proposed_keys"]
        .as_array()
        .expect("proposed contracts")
        .iter()
        .map(|key| key.as_str().expect("key"))
        .collect();
    let summary = status(&plan, false, chrono::Utc::now()).expect("summary");
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
        next_work(&plan, &NextWorkQuery::default(), chrono::Utc::now())
            .expect("next")
            .is_empty()
    );
    assert!(
        plan.decisions
            .values()
            .filter(|d| !d.blocks.is_empty())
            .all(|d| d.status == DecisionStatus::Open && d.outcome.is_none())
    );
    for item in plan.work_items.values() {
        let expected_status = if proposed.contains(item.key.0.as_str()) {
            WorkStatus::Proposed
        } else {
            WorkStatus::Planned
        };
        assert_eq!(item.execution.status, expected_status, "{}", item.key);
        assert!(item.execution.owner.is_none());
        assert_eq!(item.execution.reported_progress_percent, 0);
        if item.is_executable() {
            assert!(!item.contract.objective.trim().is_empty());
            assert!(item.contract.acceptance.len() >= 3);
            assert!(
                !item.contract.capabilities.is_empty()
                    && !item.contract.requirement_ids.is_empty()
                    && !item.execution.artifact_ids.is_empty()
            );
            if let Some(estimate) = item.schedule.estimate {
                estimate.validate().expect("estimate");
            }
            assert!(
                reaches_milestone(&plan, item.id),
                "{} has no acceptance milestone",
                item.key
            );
        } else {
            assert!(item.schedule.estimate.is_none());
            assert_eq!(item.expected_duration_hours(), 0.0);
        }
    }
    assert_eq!(plan, before);
}

#[test]
fn recorded_design_choices_are_context_and_never_execution_approval() {
    let plan = fixture();
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/self-host/dpm-alpha.expected.json"
    ))
    .expect("expected");
    let choices = expected["context_decisions"].as_object().expect("choices");
    for (key, tasks) in choices {
        let decision = plan.find_decision_by_key(key).expect("recorded choice");
        assert_eq!(decision.status, DecisionStatus::Decided);
        assert!(decision.blocks.is_empty());
        assert!(
            decision
                .rationale
                .as_ref()
                .is_some_and(|r| !r.trim().is_empty())
        );
        assert!(!decision.artifact_ids.is_empty());
        let task_ids: BTreeSet<_> = tasks
            .as_array()
            .expect("tasks")
            .iter()
            .map(|key| {
                plan.find_work_by_key(key.as_str().expect("key"))
                    .expect("task")
                    .id
            })
            .collect();
        assert_eq!(decision.related_work, task_ids);
        for id in &decision.artifact_ids {
            assert_eq!(plan.artifacts[id].metadata["role"], "planning_source");
        }
    }
    for work in plan.work_items.values().filter(|w| w.is_executable()) {
        let detail = explain_work(&plan, work.id, chrono::Utc::now()).expect("explain");
        for decision in plan
            .decisions
            .values()
            .filter(|d| choices.contains_key(&d.key.0))
        {
            assert_eq!(
                detail.context.decisions.contains(decision),
                decision.related_work.contains(&work.id)
            );
            if decision.related_work.contains(&work.id) {
                for id in &decision.artifact_ids {
                    assert!(detail.context.artifacts.iter().any(|a| a.id == *id));
                }
            }
        }
        assert!(!detail.ready);
        assert!(matches!(
            work.execution.status,
            WorkStatus::Planned | WorkStatus::Proposed
        ));
    }
    assert_eq!(plan.revision, 0);
}

#[test]
fn current_and_deferred_contracts_resolve_context_and_inherited_gates() {
    let plan = fixture();
    for key in ["MVP-10", "TUI-10", "SERVER-10"] {
        let work = plan.find_work_by_key(key).expect("work");
        let detail = explain_work(&plan, work.id, chrono::Utc::now()).expect("explain");
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
    let detail = explain_work(&plan, first.id, chrono::Utc::now()).expect("explain");
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
        let contract = work
            .contract
            .instructions
            .as_ref()
            .expect("explicit procedure");
        assert!(contract.steps.len() >= 3, "{}", work.key);
        assert!(!contract.in_scope.is_empty() && !contract.out_of_scope.is_empty());
        assert!(!contract.verification.is_empty());
        let detail = explain_work(&plan, work.id, chrono::Utc::now()).expect("explain");
        assert_eq!(detail.work.contract.instructions.as_ref(), Some(contract));
        assert_eq!(detail.work.contract.acceptance, work.contract.acceptance);
        assert!(!detail.ready);
    }
    assert_eq!(plan, before);
}

/// Tasks whose verification the declared MVP requires; execution still requires approval.
const MVP_SCOPE: [&str; 13] = [
    "MVP-10", "MVP-20", "MVP-30", "SELF-10", "CORE-20", "SCH-10", "SEM-10", "SEM-20", "SEM-30",
    "SEM-40", "RES-10", "EXT-10", "IO-10",
];
/// The live self-host cycle runs alongside the MVP without being one of its prerequisites.
const SELF_HOST_CYCLE: [&str; 2] = ["SELF-20", "SELF-30"];
const LATER_PHASE_GATES: [&str; 3] = ["DEC-POST-MVP", "DEC-LAYOUT", "DEC-EXPAND"];

fn prerequisite_tasks(plan: &Plan, target: WorkItemId) -> BTreeSet<String> {
    let mut queue = VecDeque::from([target]);
    let mut seen = BTreeSet::new();
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        queue.extend(
            plan.dependencies
                .iter()
                .filter(|d| d.successor == id)
                .map(|d| d.predecessor),
        );
    }
    seen.into_iter()
        .map(|id| &plan.work_items[&id])
        .filter(|w| w.is_executable())
        .map(|w| w.key.to_string())
        .collect()
}

#[test]
fn mvp_prerequisites_match_the_declared_scope_and_later_work_stays_gated() {
    let plan = fixture();
    let mvp = plan.find_work_by_key("M0-MVP").expect("MVP condition").id;
    let expected: BTreeSet<String> = MVP_SCOPE.iter().map(|k| (*k).to_string()).collect();
    assert_eq!(prerequisite_tasks(&plan, mvp), expected);
    for work in plan.work_items.values().filter(|w| w.is_executable()) {
        let key = work.key.to_string();
        if expected.contains(&key) || SELF_HOST_CYCLE.contains(&key.as_str()) {
            continue;
        }
        let detail = explain_work(&plan, work.id, chrono::Utc::now()).expect("explanation");
        assert!(
            detail
                .context
                .decisions
                .iter()
                .any(|d| !d.blocks.is_empty() && LATER_PHASE_GATES.contains(&d.key.0.as_str())),
            "{key} is outside the MVP but has no later-phase gate"
        );
        assert!(reaches_milestone(&plan, work.id));
        assert!(!detail.ready);
    }
}

#[test]
fn local_observation_has_no_remote_control_configuration_or_layout_prerequisites() {
    let plan = fixture();
    let local = plan.find_work_by_key("M9-LOCAL").expect("local milestone");
    let tasks = prerequisite_tasks(&plan, local.id);
    for key in [
        "UI-20", "UI-10", "UI-30", "UI-40", "UI-50", "RUN-10", "RUN-20", "QA-70", "SELF-30",
    ] {
        assert!(tasks.contains(key), "local observation needs {key}");
    }
    for key in [
        "AUTH-10",
        "AUTH-20",
        "SERVER-10",
        "SYNC-10",
        "SYNC-20",
        "COLLAB-10",
        "RUN-30",
        "UI-60",
        "CFG-10",
        "CFG-20",
        "CFG-30",
        "TUI-10",
        "TUI-20",
        "TUI-30",
        "CORE-30",
        "SCH-30",
    ] {
        assert!(
            !tasks.contains(key),
            "{key} must not delay local observation"
        );
    }
    let control = prerequisite_tasks(
        &plan,
        plan.find_work_by_key("M11-CONTROL").expect("control").id,
    );
    assert!(tasks.is_subset(&control));
    assert!(control.contains("AUTH-20") && control.contains("RUN-30"));
    assert!(!control.contains("SERVER-10"));
    let remote = prerequisite_tasks(
        &plan,
        plan.find_work_by_key("M12-REMOTE").expect("remote").id,
    );
    assert!(tasks.is_subset(&remote));
    assert!(remote.contains("AUTH-10") && remote.contains("SYNC-20"));
    assert!(!remote.contains("RUN-30"));
}

#[test]
fn unestimated_scope_and_qualification_sources_are_visible_without_false_progress() {
    let plan = fixture();
    let now = chrono::Utc::now();
    let summary = status(&plan, false, now).expect("summary");
    let unestimated: BTreeSet<_> = summary.unestimated.iter().map(|k| &k.0).collect();
    let reconciliation = plan
        .requirements
        .values()
        .find(|r| r.key.0 == "REQ-RECONCILE-001")
        .expect("reconciliation requirement")
        .id;
    for work in plan.work_items.values().filter(|w| w.is_executable()) {
        let qualification = work.execution.status == WorkStatus::Planned
            && work.contract.requirement_ids.contains(&reconciliation)
            && work.title.starts_with("Qualify");
        if work.execution.status == WorkStatus::Proposed || qualification {
            assert!(
                work.schedule.estimate.is_none(),
                "{} needs assessment",
                work.key
            );
            assert!(
                unestimated.contains(&work.key.0),
                "{} hidden uncertainty",
                work.key
            );
        }
        if qualification {
            let detail = explain_work(&plan, work.id, now).expect("qualification");
            assert!(
                detail.context.artifacts.iter().any(|a| {
                    a.uri.starts_with("git:")
                        && a.metadata.get("role").map(String::as_str) == Some("planning_source")
                }),
                "{} needs immutable implementation context",
                work.key
            );
            assert!(work.execution.events.is_empty());
            assert!(work.execution.attempts.is_empty());
            assert!(!detail.ready);
        }
    }
    assert_eq!(summary.complete, 0);
    assert_eq!(summary.progress.percent_complete, 0.0);
}

#[test]
fn proposed_execution_choices_resolve_in_the_contract_that_they_gate() {
    let plan = fixture();
    for (key, decision) in [
        ("RUN-20", "DEC-NATIVE-HOST"),
        ("UI-30", "DEC-NATIVE-HOST"),
        ("AUTH-20", "DEC-PRINCIPAL"),
        ("RUN-30", "DEC-RUN-CONTROL"),
        ("CFG-10", "DEC-CONFIG-MODEL"),
    ] {
        let work = plan.find_work_by_key(key).expect("contract");
        let detail = explain_work(&plan, work.id, chrono::Utc::now()).expect("explain");
        let choice = detail
            .context
            .decisions
            .iter()
            .find(|d| d.key.0 == decision)
            .expect("resolved gate");
        assert_eq!(choice.status, DecisionStatus::Open);
        assert!(choice.blocks.contains(&work.id));
        assert!(!detail.ready);
    }
    for key in ["RUN-10", "UI-10"] {
        let work = plan.find_work_by_key(key).expect("design input");
        let detail = explain_work(&plan, work.id, chrono::Utc::now()).expect("explain");
        let choice = detail
            .context
            .decisions
            .iter()
            .find(|d| d.key.0 == "DEC-NATIVE-HOST")
            .expect("design context");
        assert!(!choice.blocks.contains(&work.id));
        assert!(
            !detail.ready,
            "execution still needs its authorization gates"
        );
    }
}

/// Seed text is copied into live stores, where gates get decided; a sentence that states a gate's
/// current resolution would become false there, so text naming a decision must not assert its state.
#[test]
fn seed_text_naming_a_decision_stays_true_whether_or_not_it_is_decided() {
    let plan = fixture();
    let stateful = [
        "unresolved",
        "undecided",
        "is open",
        "remains open",
        "still open",
        "is decided",
        "was decided",
        "is resolved",
        "has been decided",
    ];
    for work in plan.work_items.values() {
        for text in [&work.title, &work.contract.objective] {
            let named = plan.decisions.values().any(|d| text.contains(&d.key.0));
            let lower = text.to_lowercase();
            let asserted: Vec<_> = stateful.iter().filter(|s| lower.contains(*s)).collect();
            assert!(
                !named || asserted.is_empty(),
                "{}: {text:?} asserts {asserted:?}",
                work.key
            );
        }
    }
}
