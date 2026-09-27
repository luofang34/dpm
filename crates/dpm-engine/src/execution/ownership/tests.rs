use crate::{
    Command, EngineError, NextWorkQuery, Transition, UnmetGate, apply_command, gate_report,
    is_ready, next_work,
};
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_model::{
    ActorId, Dependency, DependencyId, DependencyKind, DependencyPolicy, Plan, Release, StartBasis,
    WorkItemId, WorkStatus,
};

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

/// A -> B under one edge; no decisions, so only the relation gates execution.
fn pair(kind: DependencyKind, basis: StartBasis) -> (Plan, WorkItemId, WorkItemId, DependencyId) {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    plan.work_items.retain(|id, _| *id == a || *id == b);
    plan.decisions.clear();
    plan.risks.clear();
    let mut edge = Dependency::new(a, b, kind, 0.0);
    edge.policy = DependencyPolicy::Soft;
    edge.start_basis = basis;
    let id = edge.id;
    plan.dependencies = vec![edge];
    plan.validate().expect("pair");
    (plan, a, b, id)
}

fn fs() -> (Plan, WorkItemId, WorkItemId, DependencyId) {
    pair(DependencyKind::FinishStart, StartBasis::Verified)
}

fn first() -> ActorId {
    ActorId::agent("first")
}
fn second() -> ActorId {
    ActorId::agent("second")
}
fn lead() -> ActorId {
    ActorId::human("lead")
}

fn ok(plan: &mut Plan, actor: &ActorId, command: Command, hour: i64) {
    let label = format!("{command:?} at +{hour}h");
    apply_command(
        plan,
        actor.clone(),
        command,
        t(hour),
        dpm_model::OperationId::new(),
    )
    .unwrap_or_else(|e| panic!("{label}: {e}"));
}

/// A refused command leaves every field, including the revision, unchanged.
fn refused(plan: &mut Plan, actor: &ActorId, command: Command, hour: i64) -> EngineError {
    let before = plan.clone();
    let error = apply_command(
        plan,
        actor.clone(),
        command,
        t(hour),
        dpm_model::OperationId::new(),
    )
    .expect_err("refused");
    assert_eq!(*plan, before, "a refused command changes nothing");
    error
}

fn release(work: WorkItemId) -> Command {
    Command::Release {
        work,
        reason: "claimed the wrong task".into(),
    }
}

fn handoff(work: WorkItemId, from: ActorId, to: ActorId) -> Command {
    Command::Handoff {
        work,
        from,
        to,
        reason: "the first agent was interrupted".into(),
    }
}

fn start(plan: &mut Plan, actor: &ActorId, work: WorkItemId, hour: i64) {
    ok(plan, actor, Command::Claim { work }, hour);
    ok(plan, actor, Command::Start { work }, hour);
}

fn submit(work: WorkItemId) -> Command {
    Command::Submit { work, note: None }
}

fn listed(plan: &Plan, work: WorkItemId, hour: i64) -> bool {
    let query = NextWorkQuery {
        use_probabilistic_criticality: false,
        ..NextWorkQuery::default()
    };
    next_work(plan, &query, t(hour))
        .expect("next")
        .iter()
        .any(|c| c.work.id == work)
}

#[test]
fn an_agent_releases_its_own_claim_and_the_task_is_ready_again() {
    let (mut plan, a, _, _) = fs();
    ok(&mut plan, &first(), Command::Claim { work: a }, 0);
    assert!(!listed(&plan, a, 0));
    let revision = plan.revision;
    let operation = apply_command(
        &mut plan,
        first(),
        release(a),
        t(1),
        dpm_model::OperationId::new(),
    )
    .expect("release");
    assert_eq!(operation.base_revision, revision);
    assert!(matches!(operation.command, Command::Release { work, .. } if work == a));
    let item = &plan.work_items[&a];
    assert_eq!(
        (item.status, item.owner.clone()),
        (WorkStatus::Planned, None)
    );
    assert!(item.events.is_empty() && item.handoffs.is_empty());
    assert!(is_ready(&plan, item, t(1)) && listed(&plan, a, 1));
    ok(&mut plan, &second(), Command::Claim { work: a }, 2);
    assert_eq!(plan.work_items[&a].owner, Some(second()));
}

#[test]
fn release_is_refused_for_anyone_but_the_owner_of_an_unstarted_claim() {
    let (mut plan, a, b, _) = fs();
    assert!(matches!(
        refused(&mut plan, &first(), release(a), 0),
        EngineError::InvalidTransition {
            status: WorkStatus::Planned,
            ..
        }
    ));
    ok(&mut plan, &first(), Command::Claim { work: a }, 0);
    let blank = Command::Release {
        work: a,
        reason: " ".into(),
    };
    assert!(matches!(
        refused(&mut plan, &first(), blank, 0),
        EngineError::InvalidCommand { .. }
    ));
    for actor in [second(), lead()] {
        assert!(matches!(
            refused(&mut plan, &actor, release(a), 0),
            EngineError::OwnedByAnother { .. }
        ));
    }
    let blocked = Command::Block {
        work: a,
        reason: "waiting for access".into(),
    };
    ok(&mut plan, &first(), blocked, 0);
    assert!(matches!(
        refused(&mut plan, &first(), release(a), 0),
        EngineError::InvalidTransition {
            status: WorkStatus::Blocked,
            ..
        }
    ));
    ok(&mut plan, &first(), Command::Unblock { work: a }, 0);
    ok(&mut plan, &first(), Command::Start { work: a }, 1);
    assert!(matches!(
        refused(&mut plan, &first(), release(a), 2),
        EngineError::AlreadyStarted(_)
    ));
    ok(&mut plan, &first(), submit(a), 2);
    assert!(matches!(
        refused(&mut plan, &first(), release(a), 3),
        EngineError::AlreadyStarted(_)
    ));
    assert!(matches!(
        refused(&mut plan, &first(), release(b), 3),
        EngineError::InvalidTransition { .. }
    ));
}

#[test]
fn releasing_an_unstarted_predecessor_never_releases_ss_or_sf_successors() {
    for kind in [DependencyKind::StartStart, DependencyKind::StartFinish] {
        let (mut plan, a, b, _) = pair(kind, StartBasis::Verified);
        ok(&mut plan, &first(), Command::Claim { work: a }, 0);
        ok(&mut plan, &first(), release(a), 1);
        let transition = if kind == DependencyKind::StartStart {
            Transition::Claim
        } else {
            ok(&mut plan, &second(), Command::Claim { work: b }, 1);
            ok(&mut plan, &second(), Command::Start { work: b }, 1);
            Transition::Submit
        };
        let report = gate_report(&plan, b, transition, t(2)).expect("gates");
        assert!(!report.ready, "{kind:?}: a released claim is not a start");
        assert!(report.unmet.iter().any(|g| matches!(
            g,
            UnmetGate::Dependency {
                release: Release::AwaitingEvent,
                ..
            }
        )));
        start(&mut plan, &second(), a, 3);
        let report = gate_report(&plan, b, transition, t(3)).expect("gates");
        assert!(report.ready, "{kind:?}: the start still releases it");
    }
}

#[test]
fn only_humans_and_services_authorize_a_handoff_and_every_fact_stays() {
    let (mut plan, a, _, _) = fs();
    start(&mut plan, &first(), a, 0);
    let progress = Command::ReportProgress {
        work: a,
        percent: 40,
        note: None,
    };
    ok(&mut plan, &first(), progress, 1);
    for actor in [first(), second()] {
        assert!(matches!(
            refused(&mut plan, &actor, handoff(a, first(), second()), 2),
            EngineError::ActorNotAllowed { .. }
        ));
    }
    let before = plan.work_items[&a].clone();
    ok(&mut plan, &lead(), handoff(a, first(), second()), 2);
    let item = &plan.work_items[&a];
    assert_eq!(item.owner, Some(second()));
    assert_eq!((item.status, item.events), (before.status, before.events));
    assert_eq!(item.reported_progress_percent, 40);
    assert_eq!(
        (&item.attempts, &item.basis),
        (&before.attempts, &before.basis)
    );
    let record = item.handoffs.last().expect("handoff record");
    assert_eq!(
        (&record.from, &record.to, &record.actor),
        (&first(), &second(), &lead())
    );
    assert_eq!(record.at, t(2));
    assert!(matches!(
        refused(&mut plan, &first(), submit(a), 3),
        EngineError::OwnedByAnother { .. }
    ));
    ok(&mut plan, &second(), submit(a), 3);
    let verify = Command::Verify {
        work: a,
        note: None,
    };
    assert!(matches!(
        refused(&mut plan, &first(), verify.clone(), 4),
        EngineError::ActorNotAllowed { .. }
    ));
    assert!(matches!(
        refused(&mut plan, &second(), verify.clone(), 4),
        EngineError::SelfVerification(_)
    ));
    ok(&mut plan, &lead(), verify, 4);
}

#[test]
fn a_human_may_take_over_work_but_never_review_it_afterwards() {
    let (mut plan, a, _, _) = fs();
    ok(&mut plan, &first(), Command::Claim { work: a }, 0);
    ok(&mut plan, &lead(), handoff(a, first(), lead()), 1);
    assert_eq!(plan.work_items[&a].status, WorkStatus::Claimed);
    ok(&mut plan, &lead(), Command::Start { work: a }, 2);
    ok(&mut plan, &lead(), handoff(a, lead(), second()), 3);
    ok(&mut plan, &second(), submit(a), 4);
    let reject = Command::Reject {
        work: a,
        reason: "missing tests".into(),
    };
    for former in [lead(), first()] {
        assert!(matches!(
            refused(&mut plan, &former, reject.clone(), 5),
            EngineError::ActorNotAllowed { .. }
        ));
    }
    ok(&mut plan, &ActorId::human("reviewer"), reject, 5);
}

#[test]
fn handoff_is_refused_without_a_current_owner_or_on_submitted_work() {
    let (mut plan, a, _, _) = fs();
    assert!(matches!(
        refused(&mut plan, &lead(), handoff(a, first(), second()), 0),
        EngineError::InvalidTransition {
            status: WorkStatus::Planned,
            ..
        }
    ));
    start(&mut plan, &first(), a, 0);
    assert!(matches!(
        refused(&mut plan, &lead(), handoff(a, second(), lead()), 1),
        EngineError::OwnerMismatch { .. }
    ));
    assert!(matches!(
        refused(&mut plan, &lead(), handoff(a, first(), first()), 1),
        EngineError::InvalidCommand { .. }
    ));
    let blank = Command::Handoff {
        work: a,
        from: first(),
        to: second(),
        reason: String::new(),
    };
    assert!(matches!(
        refused(&mut plan, &lead(), blank, 1),
        EngineError::InvalidCommand { .. }
    ));
    ok(&mut plan, &lead(), handoff(a, first(), second()), 2);
    assert!(
        matches!(
            refused(&mut plan, &lead(), handoff(a, second(), first()), 1),
            EngineError::Validation(_)
        ),
        "a handoff stamped before the previous one is refused"
    );
    ok(&mut plan, &second(), submit(a), 3);
    assert!(matches!(
        refused(&mut plan, &lead(), handoff(a, second(), first()), 4),
        EngineError::InvalidTransition {
            status: WorkStatus::Submitted,
            ..
        }
    ));
    let reject = Command::Reject {
        work: a,
        reason: "rework".into(),
    };
    ok(&mut plan, &ActorId::human("reviewer"), reject, 4);
    ok(&mut plan, &lead(), handoff(a, second(), first()), 5);
    let item = &plan.work_items[&a];
    assert_eq!(item.status, WorkStatus::InProgress);
    assert_eq!(
        item.attempts.len(),
        1,
        "the rejected attempt stays in history"
    );
    assert!(item.last_rejection.is_some());
    assert_eq!(item.handoffs.len(), 2);
}

#[test]
fn blocked_work_changes_hands_with_its_blocker_and_resumes_where_it_stopped() {
    let (mut plan, a, _, _) = fs();
    start(&mut plan, &first(), a, 0);
    let block = Command::Block {
        work: a,
        reason: "credentials expired".into(),
    };
    ok(&mut plan, &first(), block, 1);
    ok(&mut plan, &lead(), handoff(a, first(), second()), 2);
    let item = &plan.work_items[&a];
    assert_eq!(item.status, WorkStatus::Blocked);
    assert_eq!(item.block_reason.as_deref(), Some("credentials expired"));
    assert!(matches!(
        refused(&mut plan, &first(), Command::Unblock { work: a }, 3),
        EngineError::OwnedByAnother { .. }
    ));
    ok(&mut plan, &second(), Command::Unblock { work: a }, 3);
    assert_eq!(plan.work_items[&a].status, WorkStatus::InProgress);
    assert_eq!(plan.work_items[&a].events.started_at, Some(t(0)));
}

#[test]
fn legacy_started_blocked_work_keeps_its_unrecorded_start_across_a_handoff() {
    let (mut plan, a, _, _) = fs();
    let item = plan.work_items.get_mut(&a).expect("a");
    item.status = WorkStatus::InProgress;
    item.owner = Some(first());
    let block = Command::Block {
        work: a,
        reason: "host down".into(),
    };
    ok(&mut plan, &first(), block, 0);
    ok(&mut plan, &lead(), handoff(a, first(), second()), 1);
    assert!(plan.work_items[&a].events.start_unrecorded);
    ok(&mut plan, &second(), Command::Unblock { work: a }, 2);
    assert_eq!(plan.work_items[&a].status, WorkStatus::InProgress);
}

#[test]
fn a_handoff_keeps_the_provisional_basis_and_its_invalidation() {
    let (mut plan, a, b, edge) = pair(DependencyKind::FinishStart, StartBasis::Provisional);
    start(&mut plan, &first(), a, 0);
    ok(&mut plan, &first(), submit(a), 1);
    start(&mut plan, &second(), b, 2);
    let basis = plan.work_items[&b].basis.clone();
    assert_eq!(basis.len(), 1);
    let builder = ActorId::agent("builder");
    ok(&mut plan, &lead(), handoff(b, second(), builder.clone()), 3);
    assert_eq!(plan.work_items[&b].basis, basis);
    let reject = Command::Reject {
        work: a,
        reason: "interface changed".into(),
    };
    ok(&mut plan, &ActorId::human("reviewer"), reject, 4);
    let report = gate_report(&plan, b, Transition::Submit, t(5)).expect("gates");
    assert!(report.unmet.iter().any(|g| matches!(
        g,
        UnmetGate::BasisInvalidated { dependency, .. } if *dependency == edge
    )));
    start_again(&mut plan, a);
    let revalidate = Command::RevalidateBasis {
        work: b,
        dependency: edge,
        attempt: 2,
        reason: "B still matches".into(),
    };
    let former = ActorId::human("former");
    ok(&mut plan, &lead(), handoff(b, builder, former.clone()), 6);
    ok(&mut plan, &lead(), handoff(b, former.clone(), second()), 6);
    assert!(matches!(
        refused(&mut plan, &former, revalidate.clone(), 7),
        EngineError::ActorNotAllowed { .. }
    ));
    ok(&mut plan, &ActorId::human("reviewer"), revalidate, 7);
}

/// Resubmit the rejected predecessor so its current attempt is #2.
fn start_again(plan: &mut Plan, a: WorkItemId) {
    ok(plan, &first(), submit(a), 5);
}

#[test]
fn a_former_owner_cannot_waive_or_restore_an_edge_of_the_work_it_held() {
    let (mut plan, a, b, edge) = fs();
    let former = ActorId::human("former");
    ok(&mut plan, &former, Command::Claim { work: a }, 0);
    ok(&mut plan, &lead(), handoff(a, former.clone(), first()), 1);
    let waive = Command::WaiveDependency {
        dependency: edge,
        reason: "not needed".into(),
    };
    assert!(matches!(
        refused(&mut plan, &former, waive.clone(), 2),
        EngineError::ActorNotAllowed { .. }
    ));
    ok(&mut plan, &lead(), waive, 2);
    assert!(is_ready(&plan, &plan.work_items[&b], t(2)));
}

#[test]
fn a_plan_change_cannot_move_owners_or_rewrite_handoffs() {
    let (mut plan, a, _, _) = fs();
    ok(&mut plan, &first(), Command::Claim { work: a }, 0);
    ok(&mut plan, &lead(), handoff(a, first(), second()), 1);
    ok(&mut plan, &second(), release(a), 2);
    reviewed_edit_refused(&mut plan, a, |w| w.handoffs.clear());
    reviewed_edit_refused(&mut plan, a, |w| w.releases.clear());
    ok(&mut plan, &first(), Command::Claim { work: a }, 3);
    reviewed_edit_refused(&mut plan, a, |w| w.owner = Some(ActorId::agent("third")));
    reviewed_edit_refused(&mut plan, a, |w| {
        let mut forged = w.handoffs[0].clone();
        forged.from = ActorId::human("reviewer");
        w.handoffs.push(forged);
    });
}

/// Ownership is changed only by claim, release and handoff, never by a reviewed plan change.
fn reviewed_edit_refused(plan: &mut Plan, work: WorkItemId, edit: fn(&mut dpm_model::WorkItem)) {
    let mut proposed = plan.clone();
    edit(proposed.work_items.get_mut(&work).expect("work"));
    let change = Command::ApplyChange {
        plan: Box::new(proposed),
        reason: "reassign".into(),
    };
    let error = refused(plan, &lead(), change, 4);
    assert!(
        matches!(error, EngineError::InvalidCommand { .. }),
        "{error:?}"
    );
}

mod independence;
