//! Independence is judged against who held the work when a review happened, and against who
//! authored its evidence.

use super::{first, fs, handoff, lead, ok, pair, refused, release, second, start, submit, t};
use crate::{Command, EngineError};
use chrono::DateTime;
use dpm_model::{
    ActorId, Artifact, ArtifactId, ArtifactKind, DependencyKind, Plan, StartBasis, WorkItemId,
};
use std::collections::BTreeMap;

fn author() -> ActorId {
    ActorId::agent("author")
}

fn artifact(created_by: ActorId, created_at: DateTime<chrono::Utc>) -> Artifact {
    Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::File,
        uri: "repo:report.md".into(),
        label: "test report".into(),
        metadata: BTreeMap::new(),
        created_by,
        created_at,
    }
}

fn attach(actor: &ActorId, work: WorkItemId, hour: i64) -> Command {
    Command::AttachArtifact {
        work,
        artifact: artifact(actor.clone(), t(hour)),
    }
}

fn reject(work: WorkItemId) -> Command {
    Command::Reject {
        work,
        reason: "acceptance check fails".into(),
    }
}

fn not_allowed(error: &EngineError) -> bool {
    matches!(error, EngineError::ActorNotAllowed { .. })
}

#[test]
fn rework_can_be_handed_to_the_reviewer_who_rejected_it_while_independent() {
    for authorizer in [lead(), ActorId::service("dispatch")] {
        let (mut plan, a, _, _) = fs();
        start(&mut plan, &first(), a, 0);
        ok(&mut plan, &first(), submit(a), 1);
        ok(&mut plan, &lead(), reject(a), 2);
        ok(&mut plan, &authorizer, handoff(a, first(), lead()), 3);
        assert_eq!(plan.work_items[&a].execution.owner, Some(lead()));
        ok(&mut plan, &lead(), submit(a), 4);
        let verify = Command::Verify {
            work: a,
            note: None,
            occurred_at: None,
        };
        assert!(not_allowed(&refused(
            &mut plan,
            &first(),
            verify.clone(),
            5
        )));
        ok(&mut plan, &ActorId::human("reviewer"), verify, 5);
    }
}

#[test]
fn a_rejection_by_an_actor_that_held_the_work_at_review_time_is_invalid() {
    let (mut plan, a, _, _) = fs();
    start(&mut plan, &first(), a, 0);
    ok(&mut plan, &lead(), handoff(a, first(), second()), 1);
    ok(&mut plan, &second(), submit(a), 2);
    ok(&mut plan, &lead(), reject(a), 3);
    let valid = plan.clone();
    for (holder, at) in [(first(), 3), (second(), 3), (second(), 1)] {
        let mut forged = valid.clone();
        let rejection = forged
            .work_items
            .get_mut(&a)
            .and_then(|w| w.execution.last_rejection.as_mut())
            .expect("rejection");
        rejection.actor = holder.clone();
        rejection.at = t(at);
        let error = forged.validate().expect_err("a holder is not independent");
        assert!(error.to_string().contains(&holder.to_string()), "{error}");
    }
    let mut earlier = valid;
    let rejection = earlier
        .work_items
        .get_mut(&a)
        .and_then(|w| w.execution.last_rejection.as_mut())
        .expect("rejection");
    rejection.actor = ActorId::human("third");
    earlier.validate().expect("a non-holder is independent");
}

#[test]
fn a_revalidation_stays_valid_after_the_successor_is_handed_to_its_reviewer() {
    let (mut plan, a, b, edge) = pair(DependencyKind::FinishStart, StartBasis::Provisional);
    start(&mut plan, &first(), a, 0);
    ok(&mut plan, &first(), submit(a), 1);
    start(&mut plan, &second(), b, 2);
    ok(&mut plan, &ActorId::human("reviewer"), reject(a), 3);
    ok(&mut plan, &first(), submit(a), 4);
    let revalidate = Command::RevalidateBasis {
        work: b,
        dependency: edge,
        attempt: 2,
        reason: "B still matches".into(),
    };
    ok(&mut plan, &lead(), revalidate, 5);
    ok(&mut plan, &lead(), handoff(b, second(), lead()), 6);
    assert_eq!(plan.work_items[&b].execution.owner, Some(lead()));
    let mut forged = plan.clone();
    let entry = forged
        .work_items
        .get_mut(&b)
        .and_then(|w| w.execution.basis.last_mut())
        .expect("revalidation");
    entry.recorded_at = t(6);
    let error = forged.validate().expect_err("the holder at that time");
    assert!(error.to_string().contains("handoff"), "{error}");
}

/// Probe from the review: the only evidence is the releaser's, so the releaser is no reviewer.
#[test]
fn a_releaser_that_attached_evidence_cannot_verify_the_result() {
    let (mut plan, a, _, _) = fs();
    let releaser = first();
    ok(&mut plan, &releaser, Command::Claim { work: a }, 0);
    ok(&mut plan, &releaser, attach(&releaser, a, 0), 0);
    ok(&mut plan, &releaser, release(a), 1);
    start(&mut plan, &second(), a, 2);
    ok(&mut plan, &second(), submit(a), 3);
    let verify = Command::Verify {
        work: a,
        note: None,
        occurred_at: None,
    };
    assert!(not_allowed(&refused(&mut plan, &releaser, verify, 4)));
    assert!(not_allowed(&refused(&mut plan, &releaser, reject(a), 4)));
}

#[test]
fn a_releaser_that_attached_nothing_is_still_a_former_holder() {
    let (mut plan, a, _, _) = fs();
    let releaser = ActorId::human("releaser");
    ok(&mut plan, &releaser, Command::Claim { work: a }, 0);
    ok(&mut plan, &releaser, release(a), 1);
    let recorded = &plan.work_items[&a].execution.releases;
    assert_eq!(recorded.len(), 1);
    assert_eq!((&recorded[0].actor, recorded[0].at), (&releaser, t(1)));
    assert!(plan.work_items[&a].held_by(&releaser));
    let mut forged = plan.clone();
    let work = forged.work_items.get_mut(&a).expect("work");
    let mut earlier = work.execution.releases[0].clone();
    earlier.at = t(0);
    work.execution.releases.push(earlier);
    assert!(forged.validate().is_err(), "releases are in time order");
    start(&mut plan, &second(), a, 2);
    ok(&mut plan, &second(), submit(a), 3);
    let verify = Command::Verify {
        work: a,
        note: None,
        occurred_at: None,
    };
    assert!(not_allowed(&refused(&mut plan, &releaser, verify, 4)));
}

/// Evidence written by someone who never held the task, such as an imported snapshot.
fn authored_by_outsider(plan: &mut Plan, work: WorkItemId) {
    let evidence = artifact(author(), t(0));
    plan.work_items
        .get_mut(&work)
        .expect("work")
        .execution
        .artifact_ids
        .insert(evidence.id);
    plan.artifacts.insert(evidence.id, evidence);
    plan.validate().expect("evidence on a task is valid");
}

#[test]
fn an_evidence_author_is_not_an_independent_reviewer() {
    let (mut plan, a, _, _) = fs();
    authored_by_outsider(&mut plan, a);
    start(&mut plan, &second(), a, 1);
    ok(&mut plan, &second(), submit(a), 2);
    let verify = Command::Verify {
        work: a,
        note: None,
        occurred_at: None,
    };
    assert!(not_allowed(&refused(
        &mut plan,
        &author(),
        verify.clone(),
        3
    )));
    assert!(not_allowed(&refused(&mut plan, &author(), reject(a), 3)));
    ok(&mut plan, &lead(), verify, 3);
}

#[test]
fn evidence_is_attached_to_a_task_only_by_its_current_owner() {
    let (mut plan, a, _, _) = fs();
    assert!(not_allowed(&refused(
        &mut plan,
        &author(),
        attach(&author(), a, 0),
        0
    )));
    ok(&mut plan, &second(), Command::Claim { work: a }, 1);
    assert!(matches!(
        refused(&mut plan, &author(), attach(&author(), a, 1), 1),
        EngineError::OwnedByAnother { .. }
    ));
    ok(&mut plan, &second(), attach(&second(), a, 1), 1);
    ok(&mut plan, &lead(), handoff(a, second(), lead()), 2);
    assert!(matches!(
        refused(&mut plan, &second(), attach(&second(), a, 2), 2),
        EngineError::OwnedByAnother { .. }
    ));
}

#[test]
fn a_planning_source_author_may_still_review_the_task() {
    let (mut plan, a, _, _) = fs();
    let mut source = artifact(author(), t(0));
    source
        .metadata
        .insert("role".into(), "planning_source".into());
    plan.work_items
        .get_mut(&a)
        .expect("work")
        .execution
        .artifact_ids
        .insert(source.id);
    plan.artifacts.insert(source.id, source);
    start(&mut plan, &second(), a, 1);
    ok(&mut plan, &second(), submit(a), 2);
    let verify = Command::Verify {
        work: a,
        note: None,
        occurred_at: None,
    };
    ok(&mut plan, &author(), verify, 3);
}

#[test]
fn evidence_cannot_be_attached_as_a_planning_source() {
    let (mut plan, a, _, _) = fs();
    ok(&mut plan, &second(), Command::Claim { work: a }, 0);
    let mut disguised = artifact(second(), t(1));
    disguised
        .metadata
        .insert("role".into(), "planning_source".into());
    let error = refused(
        &mut plan,
        &second(),
        Command::AttachArtifact {
            work: a,
            artifact: disguised,
        },
        1,
    );
    assert!(
        matches!(error, EngineError::InvalidCommand { .. }),
        "{error}"
    );
}
