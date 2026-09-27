use crate::{
    Command, EngineError, NextWorkQuery, Transition, UnmetGate, apply_command, explain_work,
    gate_report, next_work, progress, status,
};
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_model::{
    ActorId, AttemptOutcome, BasisSource, BasisState, Dependency, DependencyId, DependencyKind,
    DependencyPolicy, Plan, Release, StartBasis, WorkItemId, basis_dependents, basis_status,
};

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

struct Pair {
    plan: Plan,
    a: WorkItemId,
    b: WorkItemId,
    edge: DependencyId,
}

/// A -FS-> B with a provisional start basis and no decisions.
fn provisional(lag: f64, policy: DependencyPolicy) -> Pair {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    plan.work_items.retain(|id, _| *id == a || *id == b);
    plan.decisions.clear();
    plan.risks.clear();
    let mut edge = Dependency::new(a, b, DependencyKind::FinishStart, lag);
    edge.policy = policy;
    edge.start_basis = StartBasis::Provisional;
    let id = edge.id;
    plan.dependencies = vec![edge];
    plan.validate().expect("provisional pair");
    Pair {
        plan,
        a,
        b,
        edge: id,
    }
}

fn author() -> ActorId {
    ActorId::agent("author")
}
fn builder() -> ActorId {
    ActorId::agent("builder")
}
fn reviewer() -> ActorId {
    ActorId::human("reviewer")
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

fn revalidate(work: WorkItemId, dependency: DependencyId, attempt: u32) -> Command {
    Command::RevalidateBasis {
        work,
        dependency,
        attempt,
        reason: "B's interface use still matches A".into(),
    }
}

fn submit(work: WorkItemId) -> Command {
    Command::Submit { work, note: None }
}
fn verify(work: WorkItemId) -> Command {
    Command::Verify { work, note: None }
}
fn reject(work: WorkItemId) -> Command {
    Command::Reject {
        work,
        reason: "acceptance check fails".into(),
    }
}

/// Every view reads the shared evaluator: explain transitions equal gate reports, status counts
/// exactly the work whose finish gates report an invalidated basis, and next lists claimable work.
fn agree(plan: &Plan, hour: i64) -> Vec<UnmetGate> {
    let now = t(hour);
    let summary = status(plan, false, now).expect("status");
    let progress = progress(plan, now).expect("progress");
    let mut flagged = Vec::new();
    for work in plan.work_items.values() {
        let explained = explain_work(plan, work.id, now).expect("explain");
        for (transition, report) in &explained.transitions {
            let direct = gate_report(plan, work.id, *transition, now).expect("report");
            assert_eq!(*report, direct, "{transition} for {}", work.key);
        }
        assert_eq!(summary.gates[&work.id], explained.gates);
        assert_eq!(progress.work[&work.id], explained.progress);
        assert_eq!(explained.basis.relies_on, basis_status(plan, work));
        assert_eq!(
            explained.basis.relied_on_by,
            basis_dependents(plan, work.id)
        );
        let basis = |t: Transition| -> Vec<UnmetGate> {
            explained.transitions[&t]
                .unmet
                .iter()
                .filter(|g| matches!(g, UnmetGate::BasisInvalidated { .. }))
                .cloned()
                .collect()
        };
        assert_eq!(basis(Transition::Submit), basis(Transition::Verify));
        assert!(basis(Transition::Claim).is_empty() && basis(Transition::Start).is_empty());
        flagged.extend(basis(Transition::Verify));
    }
    let flagged_work = plan
        .work_items
        .values()
        .filter(|w| basis_status(plan, w).iter().any(|s| s.gates()))
        .count();
    assert_eq!(summary.basis_invalidated, flagged_work);
    let query = NextWorkQuery {
        use_probabilistic_criticality: false,
        ..NextWorkQuery::default()
    };
    let listed: Vec<_> = next_work(plan, &query, now)
        .expect("next")
        .into_iter()
        .map(|c| c.work.id)
        .collect();
    let claimable: Vec<_> = summary
        .gates
        .iter()
        .filter(|(_, r)| r.ready)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(listed.len(), claimable.len());
    assert!(listed.iter().all(|id| claimable.contains(id)));
    flagged
}

/// A submits attempt 1 at +1h; B claims and starts on it at +2h/+3h.
fn started_on_first_attempt(p: &mut Pair) {
    let (a, b) = (p.a, p.b);
    ok(&mut p.plan, &author(), Command::Claim { work: a }, 0);
    ok(&mut p.plan, &author(), Command::Start { work: a }, 0);
    ok(&mut p.plan, &author(), submit(a), 1);
    let claim = gate_report(&p.plan, b, Transition::Claim, t(2)).expect("claim");
    assert!(claim.ready);
    assert_eq!(
        claim
            .provisional
            .iter()
            .map(|r| r.attempt)
            .collect::<Vec<_>>(),
        [1]
    );
    ok(&mut p.plan, &builder(), Command::Claim { work: b }, 2);
    ok(&mut p.plan, &builder(), Command::Start { work: b }, 3);
    let recorded = &p.plan.work_items[&b].basis;
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        (
            recorded[0].attempt,
            &recorded[0].source,
            recorded[0].recorded_at
        ),
        (1, &BasisSource::Start, t(3))
    );
    assert!(agree(&p.plan, 3).is_empty());
}

/// After A1 is rejected at +4h, B is flagged, cannot submit, and has nothing to revalidate onto.
fn rejected_first_attempt(p: &mut Pair) {
    let b = p.b;
    ok(&mut p.plan, &reviewer(), reject(p.a), 4);
    let flagged = agree(&p.plan, 4);
    let [
        UnmetGate::BasisInvalidated {
            attempt,
            current_attempt,
            state,
            ..
        },
    ] = flagged.as_slice()
    else {
        panic!("B must be flagged after A1 is rejected");
    };
    assert_eq!((*attempt, *current_attempt), (1, None));
    assert!(
        matches!(state, BasisState::Invalidated { rejected_by, .. } if *rejected_by == reviewer())
    );
    let error = refused(&mut p.plan, &builder(), submit(b), 5);
    assert!(
        matches!(&error, EngineError::NotReady { unmet, .. } if matches!(unmet.as_slice(), [UnmetGate::BasisInvalidated { .. }])),
        "{error:?}"
    );
    assert!(
        error.to_string().contains("attempt #1, which was rejected"),
        "{error}"
    );
    let stale = refused(&mut p.plan, &reviewer(), revalidate(p.b, p.edge, 1), 5);
    assert!(
        matches!(stale, EngineError::StaleAttempt { current: None, .. }),
        "{stale:?}"
    );
}

#[test]
fn a_rejected_basis_stays_flagged_through_resubmission_and_verification_until_revalidated() {
    let mut p = provisional(0.0, DependencyPolicy::Hard);
    started_on_first_attempt(&mut p);
    rejected_first_attempt(&mut p);
    let (a, b) = (p.a, p.b);
    ok(&mut p.plan, &author(), submit(a), 6);
    assert_eq!(
        agree(&p.plan, 6).len(),
        1,
        "a new submission never validates B"
    );
    ok(&mut p.plan, &reviewer(), verify(a), 7);
    let flagged = agree(&p.plan, 7);
    assert!(
        matches!(
            flagged.as_slice(),
            [UnmetGate::BasisInvalidated {
                attempt: 1,
                current_attempt: Some(2),
                ..
            }]
        ),
        "a verification never validates B: {flagged:?}"
    );
    let outcomes: Vec<_> = p.plan.work_items[&a]
        .attempts
        .iter()
        .map(|x| &x.outcome)
        .collect();
    assert!(matches!(
        outcomes.as_slice(),
        [
            AttemptOutcome::Rejected { .. },
            AttemptOutcome::Verified { .. }
        ]
    ));
    for stale in [1, 3] {
        let error = refused(&mut p.plan, &reviewer(), revalidate(p.b, p.edge, stale), 8);
        assert!(matches!(
            error,
            EngineError::StaleAttempt {
                current: Some(2),
                ..
            }
        ));
    }

    ok(&mut p.plan, &reviewer(), revalidate(p.b, p.edge, 2), 8);
    assert!(agree(&p.plan, 8).is_empty());
    let history = &p.plan.work_items[&b].basis;
    assert_eq!(
        history.iter().map(|x| x.attempt).collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(
        matches!(&history[1].source, BasisSource::Revalidation { actor, .. } if *actor == reviewer())
    );
    let refusal = refused(&mut p.plan, &reviewer(), revalidate(p.b, p.edge, 2), 9);
    assert!(
        refusal.to_string().contains("was not rejected"),
        "{refusal}"
    );
    ok(&mut p.plan, &builder(), submit(b), 9);
    ok(&mut p.plan, &reviewer(), verify(b), 10);
    assert!(agree(&p.plan, 10).is_empty());
}

#[test]
fn repeated_rejection_needs_a_revalidation_for_every_rejected_basis() {
    let mut p = provisional(0.0, DependencyPolicy::Hard);
    started_on_first_attempt(&mut p);
    let a = p.a;
    ok(&mut p.plan, &reviewer(), reject(a), 4);
    ok(&mut p.plan, &author(), submit(a), 5);
    ok(&mut p.plan, &reviewer(), revalidate(p.b, p.edge, 2), 6);
    assert!(agree(&p.plan, 6).is_empty(), "B now relies on pending A2");
    ok(&mut p.plan, &reviewer(), reject(a), 7);
    assert!(matches!(
        agree(&p.plan, 7).as_slice(),
        [UnmetGate::BasisInvalidated {
            attempt: 2,
            current_attempt: None,
            ..
        }]
    ));
    ok(&mut p.plan, &author(), submit(a), 8);
    ok(&mut p.plan, &reviewer(), verify(a), 9);
    assert_eq!(agree(&p.plan, 9).len(), 1);
    ok(
        &mut p.plan,
        &ActorId::service("ci"),
        revalidate(p.b, p.edge, 3),
        10,
    );
    assert!(agree(&p.plan, 10).is_empty());
    let numbers: Vec<_> = p.plan.work_items[&a]
        .attempts
        .iter()
        .map(|x| x.number)
        .collect();
    assert_eq!(numbers, [1, 2, 3]);
    let relied: Vec<_> = p.plan.work_items[&p.b]
        .basis
        .iter()
        .map(|x| x.attempt)
        .collect();
    assert_eq!(relied, [1, 2, 3], "every captured basis stays inspectable");
}

#[test]
fn a_submission_never_satisfies_the_successors_verification() {
    let mut p = provisional(0.0, DependencyPolicy::Hard);
    started_on_first_attempt(&mut p);
    let (a, b) = (p.a, p.b);
    ok(&mut p.plan, &builder(), submit(b), 4);
    let error = refused(&mut p.plan, &reviewer(), verify(b), 5);
    let EngineError::NotReady { unmet, .. } = error else {
        panic!("verification must wait for A");
    };
    assert!(matches!(
        unmet.as_slice(),
        [UnmetGate::Dependency {
            release: Release::AwaitingEvent,
            accepts_submission: false,
            start_basis: StartBasis::Provisional,
            ..
        }]
    ));
    ok(&mut p.plan, &reviewer(), reject(a), 6);
    let error = refused(&mut p.plan, &reviewer(), verify(b), 7);
    assert!(error.to_string().contains("which was rejected"), "{error}");
    ok(&mut p.plan, &author(), submit(a), 8);
    ok(&mut p.plan, &reviewer(), revalidate(p.b, p.edge, 2), 9);
    refused(&mut p.plan, &reviewer(), verify(b), 10);
    ok(&mut p.plan, &reviewer(), verify(a), 11);
    ok(&mut p.plan, &reviewer(), verify(b), 12);
    assert!(agree(&p.plan, 12).is_empty());
}

#[test]
fn provisional_lag_elapses_from_the_attempt_and_a_verified_basis_needs_no_review() {
    let mut p = provisional(2.0, DependencyPolicy::Hard);
    let (a, b) = (p.a, p.b);
    ok(&mut p.plan, &author(), Command::Claim { work: a }, 0);
    ok(&mut p.plan, &author(), Command::Start { work: a }, 0);
    let awaiting = gate_report(&p.plan, b, Transition::Claim, t(1)).expect("report");
    assert!(awaiting.reasons()[0].contains("pending submission attempt"));
    ok(&mut p.plan, &author(), submit(a), 1);
    let error = refused(&mut p.plan, &builder(), Command::Claim { work: b }, 2);
    assert!(
        matches!(&error, EngineError::NotReady { unmet, .. } if matches!(unmet.as_slice(), [UnmetGate::Dependency { release: Release::Elapsing { .. }, attempt: Some(1), .. }])),
        "{error:?}"
    );
    ok(&mut p.plan, &builder(), Command::Claim { work: b }, 3);
    ok(&mut p.plan, &builder(), Command::Start { work: b }, 3);
    ok(&mut p.plan, &reviewer(), verify(a), 4);
    let [status] = basis_status(&p.plan, &p.plan.work_items[&b])
        .try_into()
        .expect("one");
    assert_eq!(status.state, BasisState::Verified);
    assert!(agree(&p.plan, 4).is_empty());
}

#[test]
fn verifying_the_attempt_never_delays_an_elapsing_provisional_start() {
    let mut p = provisional(24.0, DependencyPolicy::Hard);
    let (a, b) = (p.a, p.b);
    ok(&mut p.plan, &author(), Command::Claim { work: a }, 0);
    ok(&mut p.plan, &author(), Command::Start { work: a }, 0);
    ok(&mut p.plan, &author(), submit(a), 1);
    ok(&mut p.plan, &reviewer(), verify(a), 5);
    let report = gate_report(&p.plan, b, Transition::Claim, t(24)).expect("report");
    let [UnmetGate::Dependency { release, .. }] = report.unmet.as_slice() else {
        panic!("the lag from attempt 1 is still elapsing: {report:?}");
    };
    assert!(matches!(release, Release::Elapsing { opens_at, .. } if *opens_at == t(25)));
    assert!(agree(&p.plan, 24).is_empty());
    let open = gate_report(&p.plan, b, Transition::Claim, t(25)).expect("report");
    assert!(open.ready && open.provisional.is_empty(), "{open:?}");
    ok(&mut p.plan, &builder(), Command::Claim { work: b }, 25);
    ok(&mut p.plan, &builder(), Command::Start { work: b }, 25);
    assert!(
        p.plan.work_items[&b].basis.is_empty(),
        "a verified attempt is not a provisional basis"
    );
}

#[test]
fn revalidation_is_an_independent_human_or_service_review() {
    let mut p = provisional(0.0, DependencyPolicy::Hard);
    let (a, b) = (p.a, p.b);
    let (owner_a, owner_b) = (ActorId::human("alice"), ActorId::human("bob"));
    ok(&mut p.plan, &owner_a, Command::Claim { work: a }, 0);
    ok(&mut p.plan, &owner_a, Command::Start { work: a }, 0);
    ok(&mut p.plan, &owner_a, submit(a), 1);
    ok(&mut p.plan, &owner_b, Command::Claim { work: b }, 2);
    ok(&mut p.plan, &owner_b, Command::Start { work: b }, 2);
    ok(&mut p.plan, &reviewer(), reject(a), 3);
    ok(&mut p.plan, &owner_a, submit(a), 4);
    for actor in [owner_a, owner_b, ActorId::agent("helper")] {
        let error = refused(&mut p.plan, &actor, revalidate(p.b, p.edge, 2), 5);
        assert!(
            matches!(error, EngineError::ActorNotAllowed { .. }),
            "{actor}: {error:?}"
        );
    }
    let blank = Command::RevalidateBasis {
        work: b,
        dependency: p.edge,
        attempt: 2,
        reason: " ".into(),
    };
    refused(&mut p.plan, &reviewer(), blank, 5);
    ok(&mut p.plan, &reviewer(), revalidate(p.b, p.edge, 2), 5);
}

#[test]
fn a_waived_edge_keeps_its_invalidated_basis_as_context_only() {
    let mut p = provisional(0.0, DependencyPolicy::Soft);
    started_on_first_attempt(&mut p);
    ok(&mut p.plan, &reviewer(), reject(p.a), 4);
    assert_eq!(agree(&p.plan, 4).len(), 1);
    let waive = Command::WaiveDependency {
        dependency: p.edge,
        reason: "B no longer uses A".into(),
    };
    ok(&mut p.plan, &reviewer(), waive, 5);
    assert!(agree(&p.plan, 5).is_empty());
    let explained = explain_work(&p.plan, p.a, t(5)).expect("explain");
    let [dependent] = explained.basis.relied_on_by.as_slice() else {
        panic!("A's explanation names the affected successor");
    };
    assert!(!dependent.enforced && matches!(dependent.state, BasisState::Invalidated { .. }));
    assert_eq!(dependent.successor, p.b);
}

#[test]
fn reviewed_changes_set_the_basis_policy_only_before_execution_and_never_author_history() {
    let mut p = provisional(0.0, DependencyPolicy::Hard);
    let mut verified = p.plan.clone();
    verified.dependencies[0].start_basis = StartBasis::Verified;
    let change = |plan: &Plan| Command::ApplyChange {
        plan: Box::new(plan.clone()),
        reason: "B waits for verified A".into(),
    };
    let mut probe = p.plan.clone();
    ok(&mut probe, &reviewer(), change(&verified), 0);
    started_on_first_attempt(&mut p);
    verified = p.plan.clone();
    verified.dependencies[0].start_basis = StartBasis::Verified;
    refused(&mut p.plan, &reviewer(), change(&verified), 4);
    let mut forged = p.plan.clone();
    forged.work_items.get_mut(&p.b).expect("b").basis.clear();
    refused(&mut p.plan, &reviewer(), change(&forged), 4);
}
