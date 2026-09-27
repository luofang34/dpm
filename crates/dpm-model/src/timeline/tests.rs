use crate::*;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use std::collections::BTreeSet;

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

fn verify(plan: &mut Plan, key: &str, at: Option<DateTime<Utc>>) {
    let work = plan.find_work_by_key_mut(key).expect("work");
    work.execution.status = WorkStatus::Verified;
    work.execution.owner = Some(ActorId::agent("worker"));
    work.execution.events.verified_at = at;
}

fn gate(plan: &mut Plan, key: &str, blocks: WorkItemId, resolved: Option<DateTime<Utc>>) {
    let project = plan.work_items[&blocks].project;
    let decision = Decision {
        id: DecisionId::new(),
        key: Key::new(key),
        project,
        question: "Proceed?".into(),
        status: DecisionStatus::Decided,
        outcome: Some("Yes".into()),
        resolved_at: resolved,
        rationale: None,
        related_work: BTreeSet::new(),
        artifact_ids: BTreeSet::new(),
        blocks: BTreeSet::from([blocks]),
        supersedes: None,
        options: Vec::new(),
    };
    plan.decisions.insert(decision.id, decision);
}

#[test]
fn a_decision_resolved_after_every_prerequisite_sets_the_milestone_time() {
    let mut plan = fixture();
    let milestone = id(&plan, "TEST-M1");
    verify(&mut plan, "TEST-F", Some(t(1)));
    gate(&mut plan, "LATE-GATE", milestone, Some(t(5)));
    plan.validate().expect("valid");
    let timeline = Timeline::at(&plan, t(6));
    assert_eq!(
        timeline.completed_at(milestone),
        Some(EventTime::Recorded(t(5)))
    );
    assert_eq!(
        timeline.event(&plan, milestone, Endpoint::Start),
        timeline.event(&plan, milestone, Endpoint::Finish),
        "a zero-duration milestone starts and finishes at its reach event"
    );
    let open = plan
        .decisions
        .values_mut()
        .find(|d| d.key.0 == "LATE-GATE")
        .expect("gate");
    open.status = DecisionStatus::Open;
    open.outcome = None;
    open.resolved_at = None;
    assert!(!completion(&plan, t(6)).contains(&milestone));
}

#[test]
fn milestone_time_follows_the_latest_prerequisite_including_lag() {
    let mut plan = fixture();
    let milestone = id(&plan, "TEST-M1");
    verify(&mut plan, "TEST-F", Some(t(1)));
    plan.dependencies
        .iter_mut()
        .find(|d| d.successor == milestone)
        .expect("edge")
        .lag_hours = 24.0;
    assert!(!completion(&plan, t(24)).contains(&milestone));
    assert_eq!(
        Timeline::at(&plan, t(25)).completed_at(milestone),
        Some(EventTime::Recorded(t(25)))
    );
}

#[test]
fn legacy_verification_without_a_time_releases_zero_lag_but_not_positive_lag() {
    let mut plan = fixture();
    let milestone = id(&plan, "TEST-M1");
    verify(&mut plan, "TEST-F", None);
    let timeline = Timeline::at(&plan, t(0));
    assert_eq!(
        timeline.completed_at(milestone),
        Some(EventTime::Unrecorded)
    );
    let edge = plan
        .dependencies
        .iter_mut()
        .find(|d| d.successor == milestone)
        .expect("edge");
    edge.lag_hours = 1.0;
    let edge = edge.clone();
    let timeline = Timeline::at(&plan, t(10_000));
    assert_eq!(timeline.edge(&plan, &edge), Release::UnrecordedEventTime);
    assert_eq!(timeline.completed_at(milestone), None);
}

#[test]
fn start_events_come_from_the_start_command_or_a_started_lifecycle() {
    let mut plan = fixture();
    let a = id(&plan, "TEST-A");
    let timeline = Timeline::at(&plan, t(0));
    assert_eq!(timeline.event(&plan, a, Endpoint::Start), None);
    let work = plan.find_work_by_key_mut("TEST-A").expect("work");
    work.execution.status = WorkStatus::Claimed;
    work.execution.owner = Some(ActorId::agent("worker"));
    assert_eq!(
        Timeline::at(&plan, t(0)).event(&plan, a, Endpoint::Start),
        None,
        "a claim reserves work; it is not a start"
    );
    let work = plan.find_work_by_key_mut("TEST-A").expect("work");
    work.execution.status = WorkStatus::InProgress;
    assert_eq!(
        Timeline::at(&plan, t(0)).event(&plan, a, Endpoint::Start),
        Some(EventTime::Unrecorded)
    );
    plan.find_work_by_key_mut("TEST-A")
        .expect("work")
        .execution
        .events
        .started_at = Some(t(3));
    let timeline = Timeline::at(&plan, t(3));
    assert_eq!(
        timeline.event(&plan, a, Endpoint::Start),
        Some(EventTime::Recorded(t(3)))
    );
    assert_eq!(timeline.event(&plan, a, Endpoint::Finish), None);
}

#[test]
fn waived_edges_do_not_gate_milestones_and_packages_roll_up_child_times() {
    let mut plan = fixture();
    let milestone = id(&plan, "TEST-M1");
    let edge = plan
        .dependencies
        .iter_mut()
        .find(|d| d.successor == milestone)
        .expect("edge");
    edge.policy = DependencyPolicy::Soft;
    edge.waiver = Some(DependencyWaiver {
        actor: ActorId::human("lead"),
        at: t(0),
        reason: "scope moved".into(),
    });
    assert!(
        !completion(&plan, t(1)).contains(&milestone),
        "a milestone with no enforced prerequisite stays unreached"
    );
}

#[test]
fn event_validation_rejects_backdated_or_mismatched_times() {
    let mut plan = fixture();
    let work = plan.find_work_by_key_mut("TEST-A").expect("work");
    work.execution.status = WorkStatus::Verified;
    work.execution.owner = Some(ActorId::agent("worker"));
    work.execution.events.started_at = Some(t(5));
    work.execution.events.submitted_at = Some(t(6));
    work.execution.events.verified_at = Some(t(7));
    plan.validate().expect("ordered events");
    plan.find_work_by_key_mut("TEST-A")
        .expect("work")
        .execution
        .events
        .verified_at = Some(t(4));
    assert!(plan.validate().is_err());
    let mut plan = fixture();
    plan.find_work_by_key_mut("TEST-A")
        .expect("work")
        .execution
        .events
        .started_at = Some(t(1));
    assert!(plan.validate().is_err(), "a planned task has not started");
    let mut plan = fixture();
    plan.find_work_by_key_mut("TEST-M1")
        .expect("work")
        .execution
        .events
        .started_at = Some(t(1));
    assert!(plan.validate().is_err(), "milestones have derived events");
    let mut plan = fixture();
    plan.decisions
        .values_mut()
        .next()
        .expect("gate")
        .resolved_at = Some(t(1));
    assert!(plan.validate().is_err(), "an open gate has no resolution");
}
