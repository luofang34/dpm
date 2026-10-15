//! The schedule projection's per-task PERT figures are the classical ones of the whole three-point
//! estimate, `te = (O + 4M + P) / 6` and `σ = (P − O) / 6`, unchanged by progress or overrun, and
//! never invented for points, packages or unestimated work.
#![cfg(test)]

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{ActivityPert, Command, ScheduleProjection, apply_command, schedule_projection};
use dpm_model::{ActorId, Plan, WorkKind};

fn fixture() -> Plan {
    serde_json::from_str(include_str!("../../../tests/support/execution-plan.json"))
        .expect("fixture")
}

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

fn pert(projection: &ScheduleProjection, key: &str) -> Option<ActivityPert> {
    projection
        .work
        .iter()
        .find(|w| w.key.0 == key)
        .expect("row")
        .pert
}

#[test]
fn every_estimated_task_reports_the_classical_figures_and_points_report_none() {
    let plan = fixture();
    let projection = schedule_projection(&plan, false, t(0)).expect("projection");
    for row in &projection.work {
        let item = plan.find_work_by_key(row.key.0.as_str()).expect("item");
        match (item.kind, item.schedule.estimate) {
            (WorkKind::Task, Some(e)) => {
                let reported = row.pert.expect("an estimated task has PERT figures");
                let te = (e.optimistic_hours + 4.0 * e.likely_hours + e.pessimistic_hours) / 6.0;
                let sigma = (e.pessimistic_hours - e.optimistic_hours) / 6.0;
                assert!(
                    (reported.expected_hours - te).abs() < 1e-9,
                    "{}: te",
                    row.key
                );
                assert!(
                    (reported.sigma_hours - sigma).abs() < 1e-9,
                    "{}: σ",
                    row.key
                );
            }
            _ => assert_eq!(row.pert, None, "{} has no PERT estimate", row.key),
        }
    }
}

#[test]
fn progress_and_overrun_leave_the_figures_unchanged() {
    let mut plan = fixture();
    let before = pert(
        &schedule_projection(&plan, false, t(0)).expect("before"),
        "TEST-A",
    );
    let work = plan.find_work_by_key("TEST-A").expect("TEST-A").id;
    for command in [
        Command::claim(work),
        Command::Start {
            work,
            occurred_at: None,
        },
    ] {
        apply_command(
            &mut plan,
            ActorId::agent("worker"),
            command,
            t(0),
            dpm_model::OperationId::new(),
        )
        .expect("start");
    }
    for hours in [4, 8] {
        let during = pert(
            &schedule_projection(&plan, false, t(hours)).expect("later"),
            "TEST-A",
        );
        assert_eq!(
            during, before,
            "{hours} h into a 2–6 h task, past its pessimistic bound at 8 h"
        );
    }
    assert!(before.is_some_and(|p| p.sigma_hours > 0.0));
}
