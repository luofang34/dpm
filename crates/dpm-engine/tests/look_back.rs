//! A look back at every hour of a lifecycle with a rejection, a resubmission, a block and a handoff
//! stays a valid plan, observed through the public command API.
#![cfg(test)]

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{Command, apply_command};
use dpm_model::{ActorId, Plan, WorkStatus};

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

fn run(plan: &mut Plan, actor: &ActorId, command: Command, hour: i64) {
    let label = format!("{command:?}");
    apply_command(
        plan,
        actor.clone(),
        command,
        t(hour),
        dpm_model::OperationId::new(),
    )
    .unwrap_or_else(|e| panic!("{label}: {e}"));
}

fn status_at(plan: &Plan, hour: i64) -> (WorkStatus, Option<ActorId>, Option<DateTime<Utc>>) {
    let then = plan.as_of(t(hour) + TimeDelta::minutes(30));
    then.validate()
        .unwrap_or_else(|e| panic!("the plan at +{hour}h30: {e}"));
    let execution = &then.find_work_by_key("TEST-A").expect("work").execution;
    (
        execution.status,
        execution.owner.clone(),
        execution.events.submitted_at,
    )
}

fn first() -> ActorId {
    ActorId::agent("first")
}

fn second() -> ActorId {
    ActorId::agent("second")
}

/// TEST-A claimed at 1, started at 2, submitted at 3, rejected at 4, resubmitted at 5, rejected at
/// 6, blocked at 7 and handed from `first` to `second` at 8.
fn recorded() -> Plan {
    let mut plan: Plan =
        serde_json::from_str(include_str!("../../../tests/support/execution-plan.json"))
            .expect("fixture");
    let work = plan.find_work_by_key("TEST-A").expect("work").id;
    let (first, second, lead) = (first(), second(), ActorId::human("lead"));
    let submit = || Command::Submit {
        work,
        note: None,
        occurred_at: None,
    };
    run(&mut plan, &first, Command::claim(work), 1);
    run(
        &mut plan,
        &first,
        Command::Start {
            work,
            occurred_at: None,
        },
        2,
    );
    run(&mut plan, &first, submit(), 3);
    run(
        &mut plan,
        &lead,
        Command::Reject {
            work,
            reason: "missing evidence".into(),
        },
        4,
    );
    run(&mut plan, &first, submit(), 5);
    run(
        &mut plan,
        &lead,
        Command::Reject {
            work,
            reason: "still missing".into(),
        },
        6,
    );
    run(
        &mut plan,
        &first,
        Command::Block {
            work,
            reason: "waiting for data".into(),
        },
        7,
    );
    let handoff = Command::Handoff {
        work,
        from: first.clone(),
        to: second.clone(),
        reason: "reassigned".into(),
    };
    run(&mut plan, &lead, handoff, 8);
    plan
}

#[test]
fn every_instant_of_a_resubmitted_blocked_and_handed_off_task_is_a_valid_plan() {
    let plan = recorded();
    let (first, second) = (first(), second());
    let rows: Vec<_> = (0..9).map(|hour| status_at(&plan, hour)).collect();
    let owned = |status, at: Option<i64>| (status, Some(first.clone()), at.map(t));
    assert_eq!(rows[0], (WorkStatus::Planned, None, None));
    assert_eq!(
        rows[1],
        (WorkStatus::Planned, None, None),
        "the start replaced the claim time"
    );
    assert_eq!(rows[2], owned(WorkStatus::InProgress, None));
    assert_eq!(rows[3], owned(WorkStatus::Submitted, Some(3)));
    assert_eq!(rows[4], owned(WorkStatus::InProgress, None), "rejected");
    assert_eq!(
        rows[5],
        owned(WorkStatus::Submitted, Some(5)),
        "resubmitted"
    );
    assert_eq!(
        rows[6],
        owned(WorkStatus::Blocked, None),
        "the block follows the rejection; its own time is not recorded"
    );
    assert_eq!(
        rows[7],
        owned(WorkStatus::Blocked, None),
        "blocked before the handoff"
    );
    assert_eq!(rows[8], (WorkStatus::Blocked, Some(second), None));
}
