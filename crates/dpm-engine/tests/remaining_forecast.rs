//! The remaining forecast never releases a constraint before the execution gates do, through the
//! public API.
#![cfg(test)]

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{Command, Transition, apply_command, gate_report};
use dpm_model::{ActorId, Dependency, DependencyKind, Plan, StartBasis, WorkItemId};
use dpm_schedule::deterministic_remaining;

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

/// TEST-A -> TEST-B under one relation, with no decisions, and A started at +0h.
fn started_pair(kind: DependencyKind, lag: f64) -> (Plan, WorkItemId, WorkItemId) {
    let mut plan: Plan =
        serde_json::from_str(include_str!("../../../tests/support/execution-plan.json"))
            .expect("fixture");
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    plan.work_items.retain(|id, _| *id == a || *id == b);
    plan.decisions.clear();
    plan.risks.clear();
    plan.dependencies = vec![Dependency::new(a, b, kind, lag)];
    let owner = ActorId::agent("author");
    for command in [
        Command::Claim { work: a },
        Command::Start {
            work: a,
            occurred_at: None,
        },
    ] {
        apply_command(
            &mut plan,
            owner.clone(),
            command,
            t(0),
            dpm_model::OperationId::new(),
        )
        .expect("start A");
    }
    (plan, a, b)
}

fn forecast_start(plan: &Plan, work: WorkItemId, hour: i64) -> f64 {
    deterministic_remaining(plan, t(hour))
        .expect("remaining")
        .activities[&work]
        .earliest_start_hours
}

#[test]
fn an_in_progress_predecessor_releases_ss_lag_in_the_forecast_when_the_gate_opens() {
    let (plan, _, b) = started_pair(DependencyKind::StartStart, 10.0);
    let claim = |hour| gate_report(&plan, b, Transition::Claim, t(hour)).expect("gates");
    assert!(claim(20).ready);
    assert_eq!(forecast_start(&plan, b, 20), 0.0, "B may start now");
    assert!(!claim(4).ready);
    assert_eq!(
        forecast_start(&plan, b, 4),
        6.0,
        "the forecast opens with the gate, 6h from now"
    );
}

#[test]
fn a_provisional_start_stays_a_conservative_forecast_until_verification() {
    let (mut plan, a, b) = started_pair(DependencyKind::FinishStart, 0.0);
    plan.dependencies[0].start_basis = StartBasis::Provisional;
    let submit = Command::Submit {
        work: a,
        note: None,
        occurred_at: None,
    };
    apply_command(
        &mut plan,
        ActorId::agent("author"),
        submit,
        t(1),
        dpm_model::OperationId::new(),
    )
    .expect("submit A");
    let claim = gate_report(&plan, b, Transition::Claim, t(2)).expect("gates");
    assert!(claim.ready && !claim.provisional.is_empty());
    // A (2/4/6 h) has run for its optimistic 2 h, which rules nothing out: 4 - 2 h remain.
    let remaining = plan.work_items[&a].expected_duration_hours() - 2.0;
    assert_eq!(
        forecast_start(&plan, b, 2),
        remaining,
        "only verification finishes A in the forecast"
    );
}

#[test]
fn working_time_lags_open_the_gate_and_the_forecast_on_the_successors_calendar() {
    let (mut plan, _, b) = started_pair(DependencyKind::StartStart, 8.0);
    plan.dependencies[0].lag_basis = dpm_model::LagBasis::Working;
    plan.calendars = Some(dpm_model::Calendars::in_zone("UTC"));
    plan.work_items.get_mut(&b).expect("b").schedule.executor = Some(dpm_model::ActorKind::Human);
    let claim = |hour| gate_report(&plan, b, Transition::Claim, t(hour)).expect("gates");
    // A started Tuesday 00:00; eight Standard hours of B's calendar end Tuesday 17:00.
    assert!(!claim(16).ready);
    assert!(claim(17).ready);
    assert_eq!(
        forecast_start(&plan, b, 4),
        28.0,
        "B's work begins Wednesday 08:00"
    );
    assert_eq!(forecast_start(&plan, b, 20), 12.0);
}
