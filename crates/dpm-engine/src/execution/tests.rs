use super::*;
use crate::{NextWorkQuery, completion, next_work, show_work, status};
use dpm_model::{Dependency, DependencyKind, Key, WorkItemId, WorkKind};
use std::collections::BTreeSet;

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn task(plan: &Plan) -> WorkItemId {
    plan.find_work_by_key("TEST-A").expect("task").id
}

fn apply(plan: &mut Plan, actor: &str, command: Command) -> Operation {
    apply_command(plan, ActorId::agent(actor), command, Utc::now()).expect("command")
}

fn finish(plan: &mut Plan, work: WorkItemId) {
    apply(plan, "owner", Command::Claim { work });
    apply(plan, "owner", Command::Submit { work, note: None });
    apply(plan, "reviewer", Command::Verify { work, note: None });
}

#[test]
fn failed_commands_are_atomic_and_self_verification_is_rejected() {
    let mut plan = fixture();
    let work = task(&plan);
    apply(&mut plan, "owner", Command::Claim { work });
    for command in [
        Command::Submit { work, note: None },
        Command::Block {
            work,
            reason: "blocked".into(),
        },
    ] {
        let before = plan.clone();
        assert!(apply_command(&mut plan, ActorId::agent("other"), command, Utc::now()).is_err());
        assert_eq!(plan, before);
    }
    apply(&mut plan, "owner", Command::Submit { work, note: None });
    let before = plan.clone();
    assert!(matches!(
        apply_command(
            &mut plan,
            ActorId::agent("owner"),
            Command::Verify { work, note: None },
            Utc::now()
        ),
        Err(EngineError::SelfVerification(_))
    ));
    assert_eq!(plan, before);
    apply(&mut plan, "reviewer", Command::Verify { work, note: None });
    assert!(completion(&plan).contains(&work));
}

#[test]
fn blocking_and_resuming_preserve_the_owner() {
    let mut plan = fixture();
    let work = task(&plan);
    apply(&mut plan, "owner", Command::Claim { work });
    apply(
        &mut plan,
        "owner",
        Command::Block {
            work,
            reason: "supplier".into(),
        },
    );
    apply(&mut plan, "owner", Command::Unblock { work });
    assert_eq!(plan.work_items[&work].owner, Some(ActorId::agent("owner")));
    assert_eq!(plan.work_items[&work].status, WorkStatus::Claimed);
    assert!(
        next_work(&plan, &NextWorkQuery::default())
            .expect("query")
            .is_empty()
    );
}

#[test]
fn capabilities_filter_eligibility_instead_of_only_lowering_the_score() {
    let plan = fixture();
    let mut query = NextWorkQuery {
        capabilities: BTreeSet::from(["unrelated".into()]),
        use_probabilistic_criticality: false,
    };
    assert!(next_work(&plan, &query).expect("query").is_empty());
    query.capabilities = BTreeSet::from(["rust".into(), "testing".into()]);
    assert_eq!(next_work(&plan, &query).expect("query").len(), 1);
}

#[test]
fn revision_wraps_during_a_real_command() {
    let mut plan = fixture();
    plan.revision = u64::MAX;
    let work = task(&plan);
    let operation = apply(&mut plan, "owner", Command::Claim { work });
    assert_eq!(operation.base_revision, u64::MAX);
    assert_eq!(operation.resulting_revision, 0);
    assert_eq!(plan.revision, 0);
}

#[test]
fn milestone_completion_unlocks_successors_without_mutating_authoritative_state() {
    let mut plan = fixture();
    let work = task(&plan);
    let milestone = plan.find_work_by_key("TEST-M1").expect("milestone").id;
    plan.work_items
        .retain(|id, _| [work, milestone].contains(id));
    plan.decisions.clear();
    plan.risks.clear();
    plan.dependencies = vec![Dependency {
        predecessor: work,
        successor: milestone,
        kind: DependencyKind::FinishStart,
        lag_hours: 0.0,
    }];
    let mut successor = plan.work_items[&work].clone();
    successor.id = WorkItemId::new();
    successor.key = Key::new("AFTER");
    let after = successor.id;
    plan.work_items.insert(after, successor);
    plan.dependencies.push(Dependency {
        predecessor: milestone,
        successor: after,
        kind: DependencyKind::FinishStart,
        lag_hours: 0.0,
    });
    finish(&mut plan, work);
    assert_eq!(plan.work_items[&milestone].status, WorkStatus::Planned);
    assert_eq!(
        show_work(&plan, milestone).expect("projection").status,
        WorkStatus::Verified
    );
    assert!(is_ready(&plan, &plan.work_items[&after]));
    assert_eq!(status(&plan, false).expect("summary").complete, 2);
}

#[test]
fn empty_commands_and_invalid_plans_do_not_change_state() {
    let mut plan = fixture();
    let work = task(&plan);
    let decision = plan.find_decision_by_key("TEST-GATE").expect("gate").id;
    for command in [
        Command::Block {
            work,
            reason: " ".into(),
        },
        Command::Decide {
            decision,
            outcome: " ".into(),
        },
    ] {
        let before = plan.clone();
        assert!(apply_command(&mut plan, ActorId::human("reviewer"), command, Utc::now()).is_err());
        assert_eq!(plan, before);
    }
    plan.work_items
        .get_mut(&work)
        .expect("work")
        .acceptance
        .clear();
    let before = plan.clone();
    assert!(
        apply_command(
            &mut plan,
            ActorId::agent("owner"),
            Command::Claim { work },
            Utc::now()
        )
        .is_err()
    );
    assert_eq!(plan, before);
}

fn with_work_package(mut plan: Plan) -> (Plan, WorkItemId) {
    let mut package = plan.find_work_by_key("TEST-M1").expect("milestone").clone();
    package.id = WorkItemId::new();
    package.key = Key::new("TEST-WP");
    package.kind = WorkKind::WorkPackage;
    let id = package.id;
    plan.work_items.insert(id, package);
    plan.validate().expect("work package fixture");
    (plan, id)
}

fn execution_commands(work: WorkItemId) -> Vec<(ActorId, Command)> {
    let agent = ActorId::agent("owner");
    let reviewer = ActorId::human("reviewer");
    vec![
        (agent.clone(), Command::Claim { work }),
        (
            agent.clone(),
            Command::Block {
                work,
                reason: "waiting".into(),
            },
        ),
        (agent.clone(), Command::Unblock { work }),
        (
            agent.clone(),
            Command::ReportProgress {
                work,
                percent: 50,
                note: None,
            },
        ),
        (agent, Command::Submit { work, note: None }),
        (reviewer.clone(), Command::Verify { work, note: None }),
        (
            reviewer.clone(),
            Command::Reject {
                work,
                reason: "missing evidence".into(),
            },
        ),
        (reviewer, Command::RatifyContract { work }),
    ]
}

#[test]
fn execution_commands_on_non_tasks_fail_atomically_as_not_a_task() {
    let (plan, package) = with_work_package(fixture());
    let milestone = plan.find_work_by_key("TEST-M1").expect("milestone").id;
    for (work, kind) in [
        (milestone, WorkKind::Milestone),
        (package, WorkKind::WorkPackage),
    ] {
        for (actor, command) in execution_commands(work) {
            let mut candidate = plan.clone();
            let label = format!("{command:?}");
            let error = apply_command(&mut candidate, actor, command, Utc::now())
                .expect_err("non-task execution must fail");
            assert!(
                matches!(error, EngineError::NotATask { work: w, kind: k } if w == work && k == kind),
                "{label}: {error:?}"
            );
            assert!(error.to_string().contains("not a task"), "{label}: {error}");
            assert_eq!(candidate.revision, plan.revision, "{label}");
            assert_eq!(candidate, plan, "{label}");
        }
    }
}
