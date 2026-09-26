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
    plan.work_items.get_mut(&work).expect("task").status = WorkStatus::Proposed;
    assert!(
        next_work(&plan, &NextWorkQuery::default())
            .expect("next")
            .is_empty()
    );
    assert!(
        explain_work(&plan, work)
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
            Utc::now()
        )
        .is_err()
    );
    assert_eq!(before, plan);
    let acceptance = plan
        .work_items
        .get_mut(&work)
        .expect("task")
        .acceptance
        .split_off(0);
    let before = plan.clone();
    assert!(
        apply_command(
            &mut plan,
            ActorId::human("lead"),
            Command::RatifyContract { work },
            Utc::now()
        )
        .is_err()
    );
    assert_eq!(before, plan);
    plan.work_items.get_mut(&work).expect("task").acceptance = acceptance;
    apply_command(
        &mut plan,
        ActorId::human("lead"),
        Command::RatifyContract { work },
        Utc::now(),
    )
    .expect("ratify");
    assert!(crate::is_ready(&plan, &plan.work_items[&work]));
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
        Command::Submit { work, note: None },
    ] {
        apply_command(&mut plan, owner.clone(), command, at).expect("submit");
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
                at
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
    )
    .expect("reject");
    let item = &plan.work_items[&work];
    assert_eq!(item.status, WorkStatus::InProgress);
    assert_eq!(item.owner, Some(owner.clone()));
    assert_eq!(item.last_rejection.as_ref().expect("review").at, at);
    assert!(
        next_work(&plan, &NextWorkQuery::default())
            .expect("next")
            .is_empty()
    );
    apply_command(&mut plan, owner, Command::Submit { work, note: None }, at).expect("resubmit");
    apply_command(
        &mut plan,
        reviewer,
        Command::Verify { work, note: None },
        at,
    )
    .expect("verify");
    assert!(plan.work_items[&work].last_rejection.is_some());
}
