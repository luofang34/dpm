use crate::*;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

fn id(plan: &Plan, key: &str) -> WorkItemId {
    plan.find_work_by_key(key).expect("work").id
}

fn reviewer() -> ActorId {
    ActorId::human("reviewer")
}

fn edge_mut<'a>(plan: &'a mut Plan, from: &str, to: &str) -> &'a mut Dependency {
    let (from, to) = (id(plan, from), id(plan, to));
    plan.dependencies
        .iter_mut()
        .find(|d| d.predecessor == from && d.successor == to)
        .expect("edge")
}

/// A with rejected attempts `1..rejected` and a final attempt in `outcome`, as commands record them.
fn attempts(plan: &mut Plan, key: &str, rejected: u32, last: AttemptOutcome) {
    let work = plan.find_work_by_key_mut(key).expect("work");
    work.owner = Some(ActorId::agent("author"));
    work.events.started_at = Some(t(0));
    let rejection = |n: u32| AttemptOutcome::Rejected {
        actor: reviewer(),
        at: t(i64::from(n) * 2),
        reason: "fails".into(),
    };
    work.attempts = (1..=rejected)
        .map(|n| SubmissionAttempt {
            number: n,
            submitted_at: t(i64::from(n) * 2 - 1),
            outcome: rejection(n),
        })
        .collect();
    let submitted_at = t(i64::from(rejected) * 2 + 1);
    work.status = match &last {
        AttemptOutcome::Pending => WorkStatus::Submitted,
        AttemptOutcome::Verified { .. } => WorkStatus::Verified,
        AttemptOutcome::Rejected { .. } => WorkStatus::InProgress,
    };
    if last == AttemptOutcome::Pending {
        work.events.submitted_at = Some(submitted_at);
    }
    if let AttemptOutcome::Verified { at, .. } = &last {
        work.events.submitted_at = Some(submitted_at);
        work.events.verified_at = Some(*at);
    }
    work.attempts.push(SubmissionAttempt {
        number: rejected + 1,
        submitted_at,
        outcome: last,
    });
}

fn provisional_pair() -> Plan {
    let mut plan = fixture();
    edge_mut(&mut plan, "TEST-A", "TEST-B").start_basis = StartBasis::Provisional;
    plan.validate().expect("provisional FS between tasks");
    plan
}

#[test]
fn legacy_records_load_and_serialize_without_attempts_basis_or_policy() {
    let mut plan = fixture();
    let text = serde_json::to_string(&plan).expect("serialize");
    for field in ["attempts", "basis", "start_basis"] {
        assert!(!text.contains(field), "{field} is omitted by default");
    }
    let work = plan.find_work_by_key_mut("TEST-A").expect("a");
    work.status = WorkStatus::Submitted;
    work.owner = Some(ActorId::agent("legacy"));
    plan.validate()
        .expect("a legacy submission without attempts stays valid");
    let reloaded: Plan = serde_json::from_str(&text).expect("reload");
    assert_eq!(reloaded, fixture());
}

#[test]
fn the_provisional_policy_is_limited_to_finish_start_edges_between_tasks() {
    let plan = provisional_pair();
    let text = serde_json::to_string(&plan).expect("serialize");
    assert!(text.contains("\"start_basis\":\"Provisional\""));
    assert_eq!(serde_json::from_str::<Plan>(&text).expect("reload"), plan);
    let mut into_milestone = fixture();
    edge_mut(&mut into_milestone, "TEST-F", "TEST-M1").start_basis = StartBasis::Provisional;
    assert!(into_milestone.validate().is_err());
    let mut start_start = provisional_pair();
    edge_mut(&mut start_start, "TEST-A", "TEST-B").kind = DependencyKind::StartStart;
    assert!(start_start.validate().is_err());
}

#[test]
fn only_a_pending_attempt_releases_a_provisional_start_and_never_a_finish() {
    let mut plan = provisional_pair();
    attempts(&mut plan, "TEST-A", 0, AttemptOutcome::Pending);
    plan.validate().expect("pending attempt");
    let timeline = Timeline::at(&plan, t(2));
    let provisional = edge_mut(&mut plan.clone(), "TEST-A", "TEST-B").clone();
    let released = timeline.start_edge(&plan, &provisional);
    assert_eq!(released.attempt, Some(1));
    assert_eq!(
        released.release.released_at(),
        Some(EventTime::Recorded(t(1)))
    );
    assert_eq!(timeline.edge(&plan, &provisional), Release::AwaitingEvent);
    let verified_only = edge_mut(&mut plan.clone(), "TEST-A", "TEST-D").clone();
    assert_eq!(
        timeline.start_edge(&plan, &verified_only).release,
        Release::AwaitingEvent
    );
    assert!(timeline.completed_at(id(&plan, "TEST-A")).is_none());

    let mut legacy = provisional_pair();
    let work = legacy.find_work_by_key_mut("TEST-A").expect("a");
    work.status = WorkStatus::Submitted;
    work.owner = Some(ActorId::agent("legacy"));
    let unreferenced = Timeline::at(&legacy, t(2)).start_edge(&legacy, &provisional);
    assert_eq!(
        (unreferenced.release, unreferenced.attempt),
        (Release::AwaitingEvent, None)
    );

    let mut rejected = provisional_pair();
    let outcome = AttemptOutcome::Rejected {
        actor: reviewer(),
        at: t(2),
        reason: "fails".into(),
    };
    attempts(&mut rejected, "TEST-A", 0, outcome);
    rejected.validate().expect("rejected attempt");
    let closed = Timeline::at(&rejected, t(3)).start_edge(&rejected, &provisional);
    assert_eq!(
        (closed.release, closed.attempt),
        (Release::AwaitingEvent, None)
    );
}

#[test]
fn a_submitted_prerequisite_never_reaches_a_milestone() {
    let mut plan = fixture();
    attempts(&mut plan, "TEST-F", 0, AttemptOutcome::Pending);
    plan.validate().expect("valid");
    let milestone = id(&plan, "TEST-M1");
    assert!(
        Timeline::at(&plan, t(100))
            .completed_at(milestone)
            .is_none()
    );
}

#[test]
fn attempt_history_must_match_the_lifecycle() {
    let mut plan = fixture();
    attempts(&mut plan, "TEST-A", 2, AttemptOutcome::Pending);
    plan.validate()
        .expect("two rejections then a pending attempt");
    let a = id(&plan, "TEST-A");
    let corrupt: [fn(&mut WorkItem); 5] = [
        |w| w.attempts[0].number = 2,
        |w| w.attempts[0].outcome = AttemptOutcome::Pending,
        |w| w.status = WorkStatus::InProgress,
        |w| w.events.submitted_at = Some(t(99)),
        |w| w.attempts[2].submitted_at = t(3),
    ];
    for (index, corrupt) in corrupt.into_iter().enumerate() {
        let mut broken = plan.clone();
        corrupt(broken.work_items.get_mut(&a).expect("a"));
        assert!(
            broken.validate().is_err(),
            "corruption {index} must be rejected"
        );
    }
}

#[test]
fn a_basis_needs_a_started_successor_and_a_resolving_attempt() {
    let mut plan = provisional_pair();
    attempts(&mut plan, "TEST-A", 1, AttemptOutcome::Pending);
    let dependency = edge_mut(&mut plan, "TEST-A", "TEST-B").id;
    let (a, b) = (id(&plan, "TEST-A"), id(&plan, "TEST-B"));
    let entry = |attempt: u32, source: BasisSource| DependencyBasis {
        dependency,
        predecessor: a,
        attempt,
        recorded_at: t(2),
        source,
    };
    let successor = plan.work_items.get_mut(&b).expect("b");
    successor.basis.push(entry(1, BasisSource::Start));
    assert!(
        plan.validate().is_err(),
        "an unstarted task relies on nothing"
    );
    let successor = plan.work_items.get_mut(&b).expect("b");
    successor.status = WorkStatus::InProgress;
    successor.owner = Some(ActorId::agent("builder"));
    successor.events.started_at = Some(t(2));
    plan.validate().expect("started on attempt 1");
    let [status] = basis_status(&plan, &plan.work_items[&b])
        .try_into()
        .expect("one");
    assert!(status.gates() && status.current_attempt == Some(2));
    assert_eq!(basis_dependents(&plan, a), vec![status]);

    let revalidation = |actor: ActorId| BasisSource::Revalidation {
        actor,
        reason: "still valid".into(),
    };
    for bad in [
        entry(3, BasisSource::Start),
        entry(2, revalidation(ActorId::agent("helper"))),
        entry(2, revalidation(ActorId::agent("builder"))),
    ] {
        let mut broken = plan.clone();
        broken.work_items.get_mut(&b).expect("b").basis.push(bad);
        assert!(broken.validate().is_err());
    }
    let mut revalidated = plan.clone();
    let successor = revalidated.work_items.get_mut(&b).expect("b");
    successor.basis.push(entry(2, revalidation(reviewer())));
    revalidated.validate().expect("independent revalidation");
    let [status] = basis_status(&revalidated, &revalidated.work_items[&b])
        .try_into()
        .expect("latest basis per edge");
    assert_eq!(
        (status.basis.attempt, status.state),
        (2, BasisState::Pending)
    );
}
