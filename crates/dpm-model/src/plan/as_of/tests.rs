use crate::*;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(1945, 7, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

fn task<'a>(plan: &'a mut Plan, key: &str) -> &'a mut WorkItem {
    plan.find_work_by_key_mut(key).expect("work")
}

/// TEST-A claimed at 1, started at 2, submitted at 3 and verified at 4, as the commands record it.
fn lifecycle(plan: &mut Plan) {
    let work = task(plan, "TEST-A");
    let execution = &mut work.execution;
    execution.status = WorkStatus::Verified;
    execution.owner = Some(ActorId::agent("worker"));
    execution.reported_progress_percent = 100;
    execution.events.started_at = Some(t(2));
    execution.events.submitted_at = Some(t(3));
    execution.events.verified_at = Some(t(4));
    execution.attempts.push(SubmissionAttempt {
        number: 1,
        submitted_at: t(3),
        outcome: AttemptOutcome::Verified {
            actor: ActorId::human("reviewer"),
            at: t(4),
        },
    });
}

#[test]
fn a_task_shows_the_lifecycle_its_events_had_reached() {
    let mut plan = fixture();
    lifecycle(&mut plan);
    plan.validate().expect("valid");
    for (hours, status, owned) in [
        (1, WorkStatus::Planned, false),
        (2, WorkStatus::InProgress, true),
        (3, WorkStatus::Submitted, true),
        (4, WorkStatus::Verified, true),
    ] {
        let then = plan.as_of(t(hours));
        then.validate().expect("the plan at an instant stays valid");
        let execution = &then.find_work_by_key("TEST-A").expect("work").execution;
        assert_eq!(execution.status, status, "at hour {hours}");
        assert_eq!(execution.owner.is_some(), owned, "at hour {hours}");
        assert_eq!(
            execution.reported_progress_percent,
            if hours >= 3 { 100 } else { 0 },
            "a percentage is not placed before a later event"
        );
    }
    assert_eq!(plan.as_of(t(9)), plan, "nothing later is unchanged");
}

#[test]
fn the_rolled_back_plan_completes_what_the_timeline_completes_at_that_instant() {
    let mut plan = fixture();
    lifecycle(&mut plan);
    let gate = plan
        .decisions
        .values_mut()
        .find(|d| d.key.0 == "TEST-GATE")
        .expect("gate");
    gate.status = DecisionStatus::Decided;
    gate.outcome = Some("Proceed".into());
    gate.resolved_at = Some(t(3));
    for hours in 0..6 {
        let timeline = Timeline::at(&plan, t(hours));
        let then = plan.as_of(t(hours));
        assert_eq!(
            Timeline::at(&then, t(hours)).completed(),
            timeline.completed(),
            "at hour {hours}"
        );
        for work in plan.work_items.keys() {
            for endpoint in [Endpoint::Start, Endpoint::Finish] {
                assert_eq!(
                    Timeline::at(&then, t(hours)).event(&then, *work, endpoint),
                    timeline.event(&plan, *work, endpoint),
                    "at hour {hours}"
                );
            }
        }
    }
}

#[test]
fn a_later_handoff_or_release_leaves_the_earlier_holder_owning() {
    let mut plan = fixture();
    lifecycle(&mut plan);
    let execution = &mut task(&mut plan, "TEST-A").execution;
    execution.owner = Some(ActorId::agent("second"));
    execution.handoffs.push(Handoff {
        from: ActorId::agent("first"),
        to: ActorId::agent("second"),
        actor: ActorId::human("lead"),
        at: t(2) + TimeDelta::minutes(30),
        reason: "reassigned".into(),
    });
    let then = plan.as_of(t(2));
    let execution = &then.find_work_by_key("TEST-A").expect("work").execution;
    assert_eq!(execution.owner, Some(ActorId::agent("first")));
    assert!(execution.handoffs.is_empty());

    let mut plan = fixture();
    let execution = &mut task(&mut plan, "TEST-A").execution;
    execution.releases.push(ClaimRelease {
        actor: ActorId::agent("first"),
        at: t(5),
        reason: "cannot continue".into(),
    });
    execution.status = WorkStatus::Claimed;
    execution.owner = Some(ActorId::agent("second"));
    execution.events.claimed_at = Some(t(6));
    let then = plan.as_of(t(5) - TimeDelta::minutes(1));
    let execution = &then.find_work_by_key("TEST-A").expect("work").execution;
    assert_eq!(
        execution.status,
        WorkStatus::Planned,
        "no claim time survives"
    );
    assert_eq!(execution.owner, None);
}

#[test]
fn decisions_and_waivers_after_the_instant_had_not_happened() {
    let mut plan = fixture();
    let gate = plan
        .decisions
        .values_mut()
        .find(|d| d.key.0 == "TEST-GATE")
        .expect("gate");
    gate.status = DecisionStatus::Decided;
    gate.outcome = Some("Proceed".into());
    gate.resolved_at = Some(t(3));
    let gate = gate.id;
    let unrecorded = plan.decisions.values().find(|d| d.id != gate).map(|d| d.id);
    let soft = plan.dependencies.first_mut().expect("edge");
    soft.waiver = Some(DependencyWaiver {
        actor: ActorId::human("lead"),
        at: t(3),
        reason: "set aside".into(),
    });
    let then = plan.as_of(t(2));
    assert_eq!(then.decisions[&gate].status, DecisionStatus::Open);
    assert_eq!(then.decisions[&gate].outcome, None);
    assert!(
        then.dependencies
            .first()
            .is_some_and(|d| d.waiver.is_none())
    );
    if let Some(other) = unrecorded {
        assert_eq!(then.decisions[&other], plan.decisions[&other]);
    }
    assert_eq!(plan.as_of(t(3)), plan);
}

#[test]
fn verification_without_a_recorded_time_still_occurred() {
    let mut plan = fixture();
    lifecycle(&mut plan);
    let execution = &mut task(&mut plan, "TEST-A").execution;
    execution.events.verified_at = None;
    execution.events.started_at = None;
    execution.attempts.clear();
    execution.handoffs.push(Handoff {
        from: ActorId::agent("worker"),
        to: ActorId::agent("worker2"),
        actor: ActorId::human("lead"),
        at: t(5),
        reason: "record a later fact".into(),
    });
    let then = plan.as_of(t(1));
    assert_eq!(
        then.find_work_by_key("TEST-A")
            .expect("work")
            .execution
            .status,
        WorkStatus::Verified
    );
}
