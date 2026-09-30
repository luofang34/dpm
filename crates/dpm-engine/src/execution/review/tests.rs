use crate::{Command, NextWorkQuery, apply_command, explain_work, next_work};
use chrono::Utc;
use dpm_model::{ActorId, Plan, WorkStatus};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

#[test]
fn ratification_requires_independent_authority_and_complete_contract_atomically() {
    let mut plan = fixture();
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    plan.work_items
        .get_mut(&work)
        .expect("task")
        .execution
        .status = WorkStatus::Proposed;
    assert!(
        next_work(&plan, &NextWorkQuery::default(), chrono::Utc::now())
            .expect("next")
            .is_empty()
    );
    assert!(
        explain_work(&plan, work, chrono::Utc::now())
            .expect("explain")
            .why_now
            .iter()
            .any(|r| r == "contract not ratified")
    );
    let before = plan.clone();
    assert!(
        apply_command(
            &mut plan,
            ActorId::agent("draft"),
            Command::RatifyContract { work },
            Utc::now(),
            dpm_model::OperationId::new()
        )
        .is_err()
    );
    assert_eq!(before, plan);
    let acceptance = plan
        .work_items
        .get_mut(&work)
        .expect("task")
        .contract
        .acceptance
        .split_off(0);
    let before = plan.clone();
    assert!(
        apply_command(
            &mut plan,
            ActorId::human("lead"),
            Command::RatifyContract { work },
            Utc::now(),
            dpm_model::OperationId::new()
        )
        .is_err()
    );
    assert_eq!(before, plan);
    plan.work_items
        .get_mut(&work)
        .expect("task")
        .contract
        .acceptance = acceptance;
    apply_command(
        &mut plan,
        ActorId::human("lead"),
        Command::RatifyContract { work },
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("ratify");
    assert!(crate::is_ready(
        &plan,
        &plan.work_items[&work],
        chrono::Utc::now()
    ));
}

#[test]
fn rejection_retains_owner_and_review_across_resubmission_without_unlocking_dependents() {
    let mut plan = fixture();
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    let owner = ActorId::agent("worker");
    let reviewer = ActorId::human("reviewer");
    let at = Utc::now();
    for command in [
        Command::Claim { work },
        start_command(work),
        submit_command(work),
    ] {
        apply_command(
            &mut plan,
            owner.clone(),
            command,
            at,
            dpm_model::OperationId::new(),
        )
        .expect("submit");
    }
    for (actor, reason) in [(owner.clone(), "fix"), (reviewer.clone(), " ")] {
        let before = plan.clone();
        assert!(
            apply_command(
                &mut plan,
                actor,
                Command::Reject {
                    work,
                    reason: reason.into()
                },
                at,
                dpm_model::OperationId::new()
            )
            .is_err()
        );
        assert_eq!(before, plan);
    }
    apply_command(
        &mut plan,
        reviewer.clone(),
        Command::Reject {
            work,
            reason: "Missing acceptance evidence".into(),
        },
        at,
        dpm_model::OperationId::new(),
    )
    .expect("reject");
    let item = &plan.work_items[&work];
    assert_eq!(item.execution.status, WorkStatus::InProgress);
    assert_eq!(item.execution.owner, Some(owner.clone()));
    assert_eq!(
        item.execution.last_rejection.as_ref().expect("review").at,
        at
    );
    assert!(
        next_work(&plan, &NextWorkQuery::default(), chrono::Utc::now())
            .expect("next")
            .is_empty()
    );
    apply_command(
        &mut plan,
        owner,
        submit_command(work),
        at,
        dpm_model::OperationId::new(),
    )
    .expect("resubmit");
    apply_command(
        &mut plan,
        reviewer,
        verify_command(work),
        at,
        dpm_model::OperationId::new(),
    )
    .expect("verify");
    assert!(plan.work_items[&work].execution.last_rejection.is_some());
}

fn start_command(work: dpm_model::WorkItemId) -> Command {
    Command::Start {
        work,
        occurred_at: None,
    }
}

fn submit_command(work: dpm_model::WorkItemId) -> Command {
    Command::Submit {
        work,
        note: None,
        occurred_at: None,
    }
}

fn verify_command(work: dpm_model::WorkItemId) -> Command {
    Command::Verify {
        work,
        note: None,
        occurred_at: None,
    }
}
