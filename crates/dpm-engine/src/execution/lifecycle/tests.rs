use crate::{
    Command, EngineError, NextWorkQuery, Transition, UnmetGate, apply_command, explain_work,
    gate_report, is_ready, next_work, status,
};
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_model::{
    ActorId, Dependency, DependencyKind, DependencyPolicy, Endpoint, Plan, Release, WorkItemId,
    WorkStatus,
};

const KINDS: [DependencyKind; 4] = [
    DependencyKind::FinishStart,
    DependencyKind::StartStart,
    DependencyKind::FinishFinish,
    DependencyKind::StartFinish,
];

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

/// Two tasks joined by one edge; no decisions, so only the relation gates execution.
fn pair(
    kind: DependencyKind,
    lag: f64,
    policy: DependencyPolicy,
) -> (Plan, WorkItemId, WorkItemId) {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    plan.work_items.retain(|id, _| *id == a || *id == b);
    plan.decisions.clear();
    plan.risks.clear();
    let mut edge = Dependency::new(a, b, kind, lag);
    edge.policy = policy;
    plan.dependencies = vec![edge];
    plan.validate().expect("pair");
    (plan, a, b)
}

fn worker() -> ActorId {
    ActorId::agent("worker")
}

fn run(plan: &mut Plan, actor: &ActorId, command: Command, hour: i64) -> Result<(), EngineError> {
    apply_command(plan, actor.clone(), command, t(hour)).map(|_| ())
}

fn ok(plan: &mut Plan, actor: &ActorId, command: Command, hour: i64) {
    let label = format!("{command:?} at +{hour}h");
    run(plan, actor, command, hour).unwrap_or_else(|e| panic!("{label}: {e}"));
}

/// A refused transition names the edge that refused it and leaves every field unchanged.
fn refused(plan: &mut Plan, actor: &ActorId, command: Command, hour: i64) -> Release {
    let before = plan.clone();
    let error = run(plan, actor, command, hour).expect_err("gate must refuse");
    assert_eq!(*plan, before, "a rejected command changes nothing");
    let EngineError::NotReady { unmet, .. } = error else {
        panic!("expected a gate refusal, got {error:?}");
    };
    unmet
        .iter()
        .find_map(|g| match g {
            UnmetGate::Dependency { release, .. } => Some(*release),
            _ => None,
        })
        .expect("dependency gate")
}

/// Every query view reports the same claim readiness as the shared evaluator at this time.
fn views_agree(plan: &Plan, work: WorkItemId, hour: i64) {
    let now = t(hour);
    let claim = gate_report(plan, work, Transition::Claim, now).expect("claim report");
    let explained = explain_work(plan, work, now).expect("explain");
    assert_eq!(explained.gates, claim);
    assert_eq!(
        status(plan, false, now).expect("status").gates[&work],
        claim
    );
    assert_eq!(is_ready(plan, &plan.work_items[&work], now), claim.ready);
    let query = NextWorkQuery {
        use_probabilistic_criticality: false,
        ..NextWorkQuery::default()
    };
    let listed = next_work(plan, &query, now)
        .expect("next")
        .iter()
        .any(|c| c.work.id == work);
    assert_eq!(listed, claim.ready);
    for transition in Transition::ALL {
        assert_eq!(
            explained.transitions[&transition],
            gate_report(plan, work, transition, now).expect("report")
        );
    }
}

/// Drive the predecessor to its required event; returns that event's hour.
fn predecessor_event(plan: &mut Plan, a: WorkItemId, kind: DependencyKind) -> i64 {
    ok(plan, &worker(), Command::Claim { work: a }, 0);
    ok(plan, &worker(), Command::Start { work: a }, 0);
    if kind.predecessor_endpoint() == Endpoint::Start {
        return 0;
    }
    ok(
        plan,
        &worker(),
        Command::Submit {
            work: a,
            note: None,
        },
        1,
    );
    ok(
        plan,
        &ActorId::human("reviewer"),
        Command::Verify {
            work: a,
            note: None,
        },
        2,
    );
    2
}

#[test]
fn each_relation_gates_its_own_transition_until_24h_have_elapsed() {
    let other = ActorId::agent("other");
    let reviewer = ActorId::human("reviewer");
    for kind in KINDS {
        let (mut plan, a, b) = pair(kind, 24.0, DependencyPolicy::Hard);
        views_agree(&plan, b, 0);
        let open = predecessor_event(&mut plan, a, kind) + 24;
        views_agree(&plan, b, open - 1);
        views_agree(&plan, b, open);
        if kind.successor_endpoint() == Endpoint::Start {
            let release = refused(&mut plan, &other, Command::Claim { work: b }, open - 1);
            assert_eq!(
                release,
                Release::Elapsing {
                    event_at: t(open - 24),
                    opens_at: t(open)
                },
                "{kind:?}"
            );
            ok(&mut plan, &other, Command::Claim { work: b }, open);
            ok(&mut plan, &other, Command::Start { work: b }, open);
            ok(
                &mut plan,
                &other,
                Command::Submit {
                    work: b,
                    note: None,
                },
                open,
            );
        } else {
            ok(&mut plan, &other, Command::Claim { work: b }, 0);
            ok(&mut plan, &other, Command::Start { work: b }, 0);
            let release = refused(
                &mut plan,
                &other,
                Command::Submit {
                    work: b,
                    note: None,
                },
                open - 1,
            );
            assert!(
                matches!(release, Release::Elapsing { opens_at, .. } if opens_at == t(open)),
                "{kind:?}"
            );
            ok(
                &mut plan,
                &other,
                Command::Submit {
                    work: b,
                    note: None,
                },
                open,
            );
        }
        ok(
            &mut plan,
            &reviewer,
            Command::Verify {
                work: b,
                note: None,
            },
            open,
        );
        let events = plan.work_items[&b].events;
        assert_eq!(events.verified_at, Some(t(open)), "{kind:?}");
        assert!(events.started_at.is_some() && events.submitted_at.is_some());
    }
}

#[test]
fn restored_edges_gate_start_and_verification_of_work_reserved_or_submitted_while_waived() {
    let lead = ActorId::human("lead");
    let reviewer = ActorId::human("reviewer");
    let other = ActorId::agent("other");
    for kind in KINDS {
        let (mut plan, a, b) = pair(kind, 24.0, DependencyPolicy::Soft);
        let edge = plan.dependencies[0].id;
        let waive = Command::WaiveDependency {
            dependency: edge,
            reason: "prototype suffices".into(),
        };
        let restore = Command::RestoreDependency {
            dependency: edge,
            reason: "prototype failed".into(),
        };
        ok(&mut plan, &lead, waive, 0);
        ok(&mut plan, &other, Command::Claim { work: b }, 0);
        let finish_gate = kind.successor_endpoint() == Endpoint::Finish;
        if finish_gate {
            ok(&mut plan, &other, Command::Start { work: b }, 0);
            ok(
                &mut plan,
                &other,
                Command::Submit {
                    work: b,
                    note: None,
                },
                0,
            );
        }
        ok(&mut plan, &lead, restore, 0);
        let (gated, actor) = if finish_gate {
            (
                Command::Verify {
                    work: b,
                    note: None,
                },
                &reviewer,
            )
        } else {
            (Command::Start { work: b }, &other)
        };
        assert_eq!(
            refused(&mut plan, actor, gated.clone(), 0),
            Release::AwaitingEvent,
            "{kind:?}"
        );
        let open = predecessor_event(&mut plan, a, kind) + 24;
        assert!(matches!(
            refused(&mut plan, actor, gated.clone(), open - 1),
            Release::Elapsing { .. }
        ));
        ok(&mut plan, actor, gated, open);
    }
}

#[test]
fn a_claim_reserves_work_but_only_a_start_releases_start_to_start_successors() {
    let (mut plan, a, b) = pair(DependencyKind::StartStart, 0.0, DependencyPolicy::Hard);
    ok(&mut plan, &worker(), Command::Claim { work: a }, 0);
    assert_eq!(plan.work_items[&a].status, WorkStatus::Claimed);
    assert!(plan.work_items[&a].events.started_at.is_none());
    views_agree(&plan, b, 5);
    assert_eq!(
        refused(
            &mut plan,
            &ActorId::agent("other"),
            Command::Claim { work: b },
            5
        ),
        Release::AwaitingEvent
    );
    for command in [
        Command::Submit {
            work: a,
            note: None,
        },
        Command::ReportProgress {
            work: a,
            percent: 10,
            note: None,
        },
    ] {
        let before = plan.clone();
        let error = run(&mut plan, &worker(), command, 5).expect_err("not started");
        assert!(
            matches!(error, EngineError::NotStarted(w) if w == a),
            "{error:?}"
        );
        assert_eq!(plan, before);
    }
    ok(&mut plan, &worker(), Command::Start { work: a }, 6);
    assert_eq!(plan.work_items[&a].events.started_at, Some(t(6)));
    views_agree(&plan, b, 6);
    ok(
        &mut plan,
        &ActorId::agent("other"),
        Command::Claim { work: b },
        6,
    );
}

#[test]
fn unknown_event_times_block_only_positive_lag_with_an_actionable_reason() {
    for (kind, legacy) in [
        (DependencyKind::FinishStart, WorkStatus::Verified),
        (DependencyKind::StartStart, WorkStatus::InProgress),
    ] {
        let (mut plan, a, b) = pair(kind, 0.0, DependencyPolicy::Hard);
        let predecessor = plan.work_items.get_mut(&a).expect("a");
        predecessor.status = legacy;
        predecessor.owner = Some(worker());
        views_agree(&plan, b, 0);
        assert!(
            is_ready(&plan, &plan.work_items[&b], t(0)),
            "zero lag releases legacy {kind:?}"
        );
        plan.dependencies[0].lag_hours = 0.5;
        let report = gate_report(&plan, b, Transition::Claim, t(10_000)).expect("report");
        assert!(matches!(
            report.unmet.as_slice(),
            [UnmetGate::Dependency {
                release: Release::UnrecordedEventTime,
                ..
            }]
        ));
        let reason = report.reasons().join("\n");
        assert!(
            reason.contains("was not recorded") && reason.contains("plan change"),
            "{reason}"
        );
        assert_eq!(
            refused(
                &mut plan,
                &ActorId::agent("other"),
                Command::Claim { work: b },
                10_000
            ),
            Release::UnrecordedEventTime
        );
    }
}

#[test]
fn a_blocked_legacy_start_still_releases_start_edges_and_resumes_in_progress() {
    for kind in [DependencyKind::StartStart, DependencyKind::StartFinish] {
        let (mut plan, a, b) = pair(kind, 0.0, DependencyPolicy::Hard);
        let predecessor = plan.work_items.get_mut(&a).expect("a");
        predecessor.status = WorkStatus::InProgress;
        predecessor.owner = Some(worker());
        let release =
            |plan: &Plan| dpm_model::Timeline::at(plan, t(1)).edge(plan, &plan.dependencies[0]);
        let started = release(&plan);
        assert!(started.released_at().is_some(), "{kind:?}: {started:?}");
        let block = Command::Block {
            work: a,
            reason: "waiting on a vendor".into(),
        };
        ok(&mut plan, &worker(), block, 1);
        assert_eq!(
            release(&plan),
            started,
            "{kind:?}: blocking never undoes a start"
        );
        views_agree(&plan, b, 1);
        let report = Command::ReportProgress {
            work: a,
            percent: 40,
            note: None,
        };
        ok(&mut plan, &worker(), report, 1);
        plan.dependencies[0].lag_hours = 0.5;
        assert_eq!(release(&plan), Release::UnrecordedEventTime);
        plan.dependencies[0].lag_hours = 0.0;
        ok(&mut plan, &worker(), Command::Unblock { work: a }, 2);
        let resumed = &plan.work_items[&a];
        assert_eq!(resumed.status, WorkStatus::InProgress, "{kind:?}");
        assert_eq!(resumed.events.started_at, None, "no start time is invented");
        let submit = Command::Submit {
            work: a,
            note: None,
        };
        ok(&mut plan, &worker(), submit, 3);
    }
}

#[test]
fn a_lead_shapes_the_schedule_but_never_releases_work_before_the_event() {
    let (mut plan, a, b) = pair(DependencyKind::FinishStart, -48.0, DependencyPolicy::Hard);
    let schedule = dpm_schedule::deterministic_remaining(&plan, t(0)).expect("schedule");
    assert!(
        schedule.activities[&b].earliest_start_hours
            < schedule.activities[&a].earliest_finish_hours
    );
    assert_eq!(
        refused(
            &mut plan,
            &ActorId::agent("other"),
            Command::Claim { work: b },
            100
        ),
        Release::AwaitingEvent
    );
    let why = explain_work(&plan, b, t(100))
        .expect("explain")
        .why_now
        .join("\n");
    assert!(
        why.contains("lead") && why.contains("schedule only"),
        "{why}"
    );
    let open = predecessor_event(&mut plan, a, DependencyKind::FinishStart);
    ok(
        &mut plan,
        &ActorId::agent("other"),
        Command::Claim { work: b },
        open,
    );
}

fn submit(work: WorkItemId) -> Command {
    Command::Submit { work, note: None }
}

fn verify(work: WorkItemId) -> Command {
    Command::Verify { work, note: None }
}

#[test]
fn event_times_are_the_command_times_and_cannot_be_backdated() {
    let (mut plan, a, _) = pair(DependencyKind::FinishStart, 0.0, DependencyPolicy::Hard);
    let reviewer = ActorId::human("reviewer");
    ok(&mut plan, &worker(), Command::Claim { work: a }, 3);
    ok(&mut plan, &worker(), Command::Start { work: a }, 5);
    let before = plan.clone();
    assert!(run(&mut plan, &worker(), submit(a), 4).is_err());
    assert_eq!(plan, before, "a submission before the recorded start");
    ok(&mut plan, &worker(), submit(a), 8);
    let before = plan.clone();
    assert!(run(&mut plan, &reviewer, verify(a), 7).is_err());
    assert_eq!(plan, before, "a verification before the submission");
    ok(&mut plan, &reviewer, verify(a), 11);
    let events = plan.work_items[&a].events;
    assert_eq!(
        (events.started_at, events.submitted_at, events.verified_at),
        (Some(t(5)), Some(t(8)), Some(t(11)))
    );
}

#[test]
fn blocking_and_rejection_keep_the_start_and_drop_the_rejected_submission() {
    let (mut plan, a, _) = pair(DependencyKind::FinishStart, 0.0, DependencyPolicy::Hard);
    let reviewer = ActorId::human("reviewer");
    ok(&mut plan, &worker(), Command::Claim { work: a }, 3);
    ok(&mut plan, &worker(), Command::Start { work: a }, 5);
    let block = Command::Block {
        work: a,
        reason: "vendor".into(),
    };
    ok(&mut plan, &worker(), block, 6);
    let report = Command::ReportProgress {
        work: a,
        percent: 40,
        note: None,
    };
    ok(&mut plan, &worker(), report, 6);
    ok(&mut plan, &worker(), Command::Unblock { work: a }, 7);
    let resumed = &plan.work_items[&a];
    assert_eq!(
        resumed.status,
        WorkStatus::InProgress,
        "started work resumes started"
    );
    assert_eq!(resumed.events.started_at, Some(t(5)));
    ok(&mut plan, &worker(), submit(a), 8);
    let reject = Command::Reject {
        work: a,
        reason: "missing test".into(),
    };
    ok(&mut plan, &reviewer, reject, 9);
    assert_eq!(plan.work_items[&a].events.submitted_at, None);
    ok(&mut plan, &worker(), submit(a), 10);
    assert_eq!(plan.work_items[&a].events.submitted_at, Some(t(10)));
}
