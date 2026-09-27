use super::*;
use crate::{Command, apply_command, explain_work};
use chrono::Utc;
use dpm_model::{ActorId, Key, WorkKind};
use std::collections::BTreeSet;

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
    assert!(!is_ready(
        &plan,
        &plan.work_items[&task],
        chrono::Utc::now()
    ));
    assert!(
        explain_work(&plan, task, chrono::Utc::now())
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
        dpm_model::OperationId::new(),
    )
    .expect("decide");
    assert!(is_ready(&plan, &plan.work_items[&task], chrono::Utc::now()));
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
        ("worker", Command::Start { work: task }),
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
        apply_command(
            &mut plan,
            ActorId::agent(actor),
            command,
            Utc::now(),
            dpm_model::OperationId::new(),
        )
        .expect("execute");
    }
    assert_eq!(
        completion(&plan, chrono::Utc::now()),
        BTreeSet::from([task, parent, outer_id])
    );
    assert_eq!(
        show_work(&plan, outer_id, chrono::Utc::now())
            .expect("projection")
            .status,
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
    assert!(!is_ready(
        &plan,
        &plan.work_items[&task],
        chrono::Utc::now()
    ));
    assert!(!completion(&plan, chrono::Utc::now()).contains(&parent));
    plan.work_items.get_mut(&parent).expect("package").kind = WorkKind::Milestone;
    assert!(!completion(&plan, chrono::Utc::now()).contains(&parent));
}

#[test]
fn readiness_rejects_foreign_stale_or_invalid_contracts() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let work = plan.find_work_by_key("TEST-A").expect("work").clone();
    assert!(is_ready(&plan, &work, chrono::Utc::now()));
    let mut foreign = work.clone();
    foreign.id = WorkItemId::new();
    assert!(!is_ready(&plan, &foreign, chrono::Utc::now()));
    apply_command(
        &mut plan,
        ActorId::agent("owner"),
        Command::Claim { work: work.id },
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("claim");
    assert!(!is_ready(&plan, &work, chrono::Utc::now()));
    let stored = plan.work_items.get_mut(&work.id).expect("work");
    stored.status = WorkStatus::Planned;
    stored.owner = None;
    stored.acceptance.clear();
    let invalid = stored.clone();
    assert!(!is_ready(&plan, &invalid, chrono::Utc::now()));
    let before = plan.clone();
    assert!(
        apply_command(
            &mut plan,
            ActorId::agent("owner"),
            Command::Claim { work: work.id },
            Utc::now(),
            dpm_model::OperationId::new()
        )
        .is_err()
    );
    assert_eq!(plan, before);
}

fn at(hours: i64) -> chrono::DateTime<Utc> {
    use chrono::TimeZone;
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("time")
        + chrono::TimeDelta::hours(hours)
}

/// `A -FS-> M -FS+24h-> B`, where decision gate G blocks milestone M and A is verified at +2h.
fn gated_milestone() -> (Plan, [WorkItemId; 3], dpm_model::DecisionId) {
    use dpm_model::{Dependency, DependencyKind};
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let ids = ["TEST-A", "TEST-B", "TEST-M1"].map(|k| plan.find_work_by_key(k).expect("work").id);
    let [a, b, milestone] = ids;
    plan.work_items.retain(|id, _| ids.contains(id));
    plan.risks.clear();
    plan.dependencies = vec![
        Dependency::new(a, milestone, DependencyKind::FinishStart, 0.0),
        Dependency::new(milestone, b, DependencyKind::FinishStart, 24.0),
    ];
    let gate = plan.decisions.values_mut().next().expect("gate");
    gate.blocks = BTreeSet::from([milestone]);
    let gate = gate.id;
    let (worker, reviewer) = (ActorId::agent("worker"), ActorId::human("reviewer"));
    for (actor, command, hour) in [
        (&worker, Command::Claim { work: a }, 0),
        (&worker, Command::Start { work: a }, 0),
        (
            &worker,
            Command::Submit {
                work: a,
                note: None,
            },
            1,
        ),
        (
            &reviewer,
            Command::Verify {
                work: a,
                note: None,
            },
            2,
        ),
    ] {
        apply_command(
            &mut plan,
            actor.clone(),
            command,
            at(hour),
            dpm_model::OperationId::new(),
        )
        .expect("predecessor");
    }
    (plan, ids, gate)
}

#[test]
fn a_decision_approved_after_every_verification_sets_the_milestone_time_in_every_view() {
    let (mut plan, [_, b, milestone], gate) = gated_milestone();
    assert!(
        !completion(&plan, at(9)).contains(&milestone),
        "the open gate holds it"
    );
    let decide = Command::Decide {
        decision: gate,
        outcome: "accepted".into(),
    };
    apply_command(
        &mut plan,
        ActorId::human("lead"),
        decide,
        at(10),
        dpm_model::OperationId::new(),
    )
    .expect("decide");
    let reached = Some(dpm_model::EventTime::Recorded(at(10)));
    let timeline = dpm_model::Timeline::at(&plan, at(10));
    assert_eq!(timeline.completed_at(milestone), reached);
    let progress = crate::progress(&plan, at(10)).expect("progress");
    assert_eq!(progress.work[&milestone].completed_at, reached);
    let explained = explain_work(&plan, milestone, at(10)).expect("explain");
    assert_eq!(explained.progress.completed_at, reached);
    let shown = show_work(&plan, milestone, at(10)).expect("show");
    assert_eq!(shown.status, WorkStatus::Verified);
    assert_eq!(
        crate::status(&plan, false, at(10))
            .expect("status")
            .complete,
        2
    );
    let worker = ActorId::agent("worker");
    let before = plan.clone();
    let refused = apply_command(
        &mut plan,
        worker.clone(),
        Command::Claim { work: b },
        at(33),
        dpm_model::OperationId::new(),
    );
    assert!(
        matches!(refused, Err(crate::EngineError::NotReady { .. })),
        "{refused:?}"
    );
    assert_eq!(plan, before, "+23h after the decision is too early");
    assert!(!is_ready(&plan, &plan.work_items[&b], at(33)));
    assert!(is_ready(&plan, &plan.work_items[&b], at(34)));
    apply_command(
        &mut plan,
        worker,
        Command::Claim { work: b },
        at(34),
        dpm_model::OperationId::new(),
    )
    .expect("+24h");
}
