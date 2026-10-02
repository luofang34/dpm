//! Derived observation, orphaning and operation links.

use super::*;
use chrono::TimeDelta;
use dpm_model::{ActivityKind, LatestActivity, LineageStatus, ObservedStatus, Orphan};

#[test]
fn silence_makes_an_unfinished_run_stale_and_never_finished() {
    let (plan, work) = claimed();
    let (record, lineage) = started(&plan, work);
    let at = record.started_at;
    let view = |state, now| {
        project(
            snapshot(&record, state, at),
            Some(&plan),
            Some(lineage),
            now,
        )
    };
    let fresh = view(RunState::Working, at + TimeDelta::seconds(299));
    assert_eq!(fresh.status, ObservedStatus::Working);
    assert_eq!(fresh.stale_at, Some(at + STALE_AFTER));
    assert_eq!(fresh.state, RunState::Working);
    let stale = view(RunState::Working, at + STALE_AFTER);
    assert_eq!(stale.status, ObservedStatus::Stale);
    assert_eq!(
        stale.state,
        RunState::Working,
        "silence leaves the lifecycle fact alone"
    );
    assert_eq!(stale.stale_at, None);
    assert_eq!(
        view(RunState::Waiting, at + TimeDelta::seconds(1)).status,
        ObservedStatus::Waiting
    );
    assert_eq!(
        view(RunState::Waiting, at + TimeDelta::days(30)).status,
        ObservedStatus::Stale
    );
}

#[test]
fn terminal_states_are_reported_as_recorded_however_long_ago() {
    let (plan, work) = claimed();
    let (record, lineage) = started(&plan, work);
    let later = record.started_at + TimeDelta::days(30);
    for (state, status) in [
        (RunState::Failed, ObservedStatus::Failed),
        (RunState::Interrupted, ObservedStatus::Interrupted),
        (RunState::Completed, ObservedStatus::Completed),
    ] {
        let view = project(
            snapshot(&record, state, record.started_at),
            Some(&plan),
            Some(lineage),
            later,
        );
        assert_eq!(
            (view.status, view.stale_at, view.orphan),
            (status, None, None)
        );
    }
}

#[test]
fn activity_receipts_keep_a_run_fresh_and_a_reported_time_does_not() {
    let (plan, work) = claimed();
    let (record, lineage) = started(&plan, work);
    let start = record.started_at;
    let mut shot = snapshot(&record, RunState::Working, start);
    shot.activity = ActivityTally {
        recorded: 3,
        retained: 0,
        source_high_water: 3,
        latest: Some(LatestActivity {
            kind: ActivityKind::Heartbeat,
            source_sequence: 3,
            recorded_at: start + TimeDelta::seconds(250),
        }),
    };
    let now = start + TimeDelta::seconds(400);
    let view = project(shot, Some(&plan), Some(lineage), now);
    assert_eq!(view.status, ObservedStatus::Working, "received 150 s ago");
    assert_eq!(view.last_receipt_at, start + TimeDelta::seconds(250));
    assert_eq!(view.stale_at, Some(start + TimeDelta::seconds(550)));
    // Retention may have removed every record, but the tally still knows the last receipt.
    assert_eq!(view.activity.retained, 0);
    // A time the executor claims about itself is never consulted.
    let mut boastful = snapshot(&record, RunState::Working, start);
    boastful.last.observed_at = Some(now + TimeDelta::days(3650));
    let view = project(boastful, Some(&plan), Some(lineage), now);
    assert_eq!(view.status, ObservedStatus::Stale);
}

#[test]
fn a_run_from_another_lineage_is_unknown_and_keeps_its_attribution() {
    let (plan, work) = claimed();
    let (record, lineage) = started(&plan, work);
    let snapshot = snapshot(&record, RunState::Working, record.started_at);
    let restored = Some(LineageId::new());
    let view = project(snapshot.clone(), Some(&plan), restored, record.started_at);
    assert_eq!(view.status, ObservedStatus::Unknown);
    assert_eq!(view.lineage, LineageStatus::Foreign);
    assert_eq!(view.run.contract.lineage_id, lineage);
    assert_eq!(
        view.orphan, None,
        "another history's plan says nothing about it"
    );
    let finished = RunSnapshot {
        last: LifecycleEvent {
            state: RunState::Completed,
            ..snapshot.last
        },
        ..snapshot
    };
    let view = project(finished, Some(&plan), restored, record.started_at);
    assert_eq!(view.status, ObservedStatus::Completed);
    assert_eq!(view.lineage, LineageStatus::Foreign);
}

#[test]
fn an_unfinished_run_whose_task_moved_on_is_reported_orphaned() {
    let (mut plan, work) = claimed();
    let (record, lineage) = started(&plan, work);
    let at = record.started_at;
    let orphan = |plan: &Plan| {
        project(
            snapshot(&record, RunState::Working, at),
            Some(plan),
            Some(lineage),
            at,
        )
        .orphan
    };
    assert_eq!(orphan(&plan), None);
    apply_command(
        &mut plan,
        ActorId::human("lead"),
        Command::Handoff {
            work,
            from: worker(),
            to: ActorId::agent("heir"),
            reason: "rebalance".into(),
        },
        Utc::now(),
        OperationId::new(),
    )
    .expect("handoff");
    assert_eq!(
        orphan(&plan),
        Some(Orphan::OwnerChanged {
            owner: Some(ActorId::agent("heir"))
        })
    );
    let mut blocked = plan.clone();
    let item = blocked.work_items.get_mut(&work).expect("work");
    item.execution.owner = Some(worker());
    item.execution.status = dpm_model::WorkStatus::Submitted;
    assert_eq!(
        orphan(&blocked),
        Some(Orphan::NotExecuting {
            status: dpm_model::WorkStatus::Submitted
        })
    );
    blocked.work_items.remove(&work);
    assert_eq!(orphan(&blocked), Some(Orphan::WorkMissing));
}

fn facts(record: &RunRecord) -> OperationFacts {
    OperationFacts {
        id: OperationId::new(),
        actor: record.executor.clone(),
        timestamp: record.started_at + TimeDelta::seconds(5),
        work: Some(record.work),
        workspace: record.contract.workspace_id,
        lineage: record.contract.lineage_id,
    }
}

#[test]
fn an_operation_links_only_when_it_is_provably_the_runs_own() {
    let (plan, work) = claimed();
    let (record, _) = started(&plan, work);
    check_link(&record, None, &facts(&record)).expect("the executor's operation on its task");
    let refusal = |facts: OperationFacts, ended| match check_link(&record, ended, &facts) {
        Err(RunError::LinkRefused { reason, .. }) => reason,
        other => panic!("expected a refusal, got {other:?}"),
    };
    let own = facts(&record);
    assert_eq!(
        refusal(
            OperationFacts {
                workspace: dpm_model::WorkspaceId::new(),
                ..own.clone()
            },
            None
        ),
        LinkRefusal::OtherWorkspace
    );
    assert_eq!(
        refusal(
            OperationFacts {
                lineage: LineageId::new(),
                ..own.clone()
            },
            None
        ),
        LinkRefusal::OtherLineage
    );
    assert_eq!(
        refusal(
            OperationFacts {
                work: Some(WorkItemId::new()),
                ..own.clone()
            },
            None
        ),
        LinkRefusal::OtherWork
    );
    assert_eq!(
        refusal(
            OperationFacts {
                work: None,
                ..own.clone()
            },
            None
        ),
        LinkRefusal::OtherWork
    );
    assert_eq!(
        refusal(
            OperationFacts {
                actor: ActorId::human("lead"),
                ..own.clone()
            },
            None
        ),
        LinkRefusal::OtherActor
    );
    assert_eq!(
        refusal(
            OperationFacts {
                timestamp: record.started_at - TimeDelta::seconds(1),
                ..own.clone()
            },
            None
        ),
        LinkRefusal::BeforeStart
    );
    assert_eq!(
        refusal(own.clone(), Some(own.timestamp - TimeDelta::seconds(1))),
        LinkRefusal::AfterEnd
    );
    check_link(&record, Some(own.timestamp), &own).expect("at the end instant");
}

#[test]
fn operations_of_every_command_are_classified_by_the_task_they_act_on() {
    let (mut plan, work) = claimed();
    let operation = apply_command(
        &mut plan,
        worker(),
        Command::Start {
            work,
            occurred_at: None,
        },
        Utc::now(),
        OperationId::new(),
    )
    .expect("start");
    let found = OperationFacts::of(&operation, plan.workspace.id, LineageId::new());
    assert_eq!((found.work, found.actor), (Some(work), worker()));
    let decide = crate::Command::Decide {
        decision: dpm_model::DecisionId::new(),
        outcome: "go".into(),
    };
    let operation = crate::Operation {
        command: decide,
        ..operation
    };
    assert_eq!(
        OperationFacts::of(&operation, plan.workspace.id, LineageId::new()).work,
        None
    );
}
