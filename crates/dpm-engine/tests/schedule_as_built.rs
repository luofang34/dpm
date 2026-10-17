//! The as-built critical path of work recorded long ago: the recorded relation with the smallest gap
//! drives its successor, and the chain of driving relations ending at the last recorded finish is
//! critical, while the remaining schedule of finished work is all zero and says nothing.
#![cfg(test)]

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{Command, ScheduleProjection, apply_command, schedule_projection};
use dpm_model::{ActorId, OperationId, Plan};

fn day(month: u32, day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(1950, month, day, 0, 0, 0)
        .single()
        .expect("fixed time")
}

/// The recorded plan finished in January 1950, as the qualification records it: REC-A, then REC-B
/// and REC-C after it, REC-D after both (REC-B finishes last), REC-E started with REC-A.
fn finished() -> Plan {
    let mut plan: Plan =
        serde_json::from_str(include_str!("../../../tests/support/recorded-plan.json"))
            .expect("fixture");
    let mut commit = Utc
        .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .single()
        .expect("commit time");
    for (key, start, finish) in [
        ("REC-A", day(1, 1), day(1, 10)),
        ("REC-E", day(1, 3), day(1, 5)),
        ("REC-B", day(1, 10), day(1, 20)),
        ("REC-C", day(1, 12), day(1, 15)),
        ("REC-D", day(1, 20), day(2, 1)),
    ] {
        let work = plan.find_work_by_key(key).expect("task").id;
        let worker = ActorId::agent("worker");
        for (actor, command) in [
            (
                worker.clone(),
                Command::Claim {
                    work,
                    occurred_at: Some(start),
                },
            ),
            (
                worker.clone(),
                Command::Start {
                    work,
                    occurred_at: Some(start),
                },
            ),
            (
                worker,
                Command::Submit {
                    work,
                    note: None,
                    occurred_at: Some(finish),
                },
            ),
            (
                ActorId::human("lead"),
                Command::Verify {
                    work,
                    note: None,
                    occurred_at: Some(finish),
                },
            ),
        ] {
            commit += TimeDelta::minutes(1);
            apply_command(&mut plan, actor, command, commit, OperationId::new()).expect("recorded");
        }
    }
    plan
}

fn projection() -> ScheduleProjection {
    let now = Utc
        .with_ymd_and_hms(2026, 10, 17, 0, 0, 0)
        .single()
        .expect("clock");
    schedule_projection(&finished(), false, now).expect("projection")
}

fn key(projection: &ScheduleProjection, id: dpm_model::WorkItemId) -> String {
    projection
        .work
        .iter()
        .find(|w| w.id == id)
        .expect("row")
        .key
        .0
        .clone()
}

#[test]
fn the_chain_that_drove_the_last_finish_is_critical_as_built() {
    let projection = projection();
    let critical: Vec<String> = projection
        .work
        .iter()
        .filter(|w| w.actual.is_some_and(|a| a.critical))
        .map(|w| w.key.0.clone())
        .collect();
    assert_eq!(critical, ["REC-A", "REC-B", "REC-D"]);
    assert!(
        projection.relations.is_empty(),
        "nothing outstanding binds anything"
    );
}

#[test]
fn every_recorded_relation_reports_its_gap_and_whether_it_drove() {
    let projection = projection();
    let found: Vec<(String, String, f64, bool, bool)> = projection
        .recorded_relations
        .iter()
        .map(|r| {
            (
                key(&projection, r.predecessor),
                key(&projection, r.successor),
                r.gap_hours,
                r.driving,
                r.critical,
            )
        })
        .collect();
    let expected = [
        ("REC-A", "REC-B", 0.0, true, true),
        ("REC-A", "REC-C", 48.0, true, false),
        ("REC-B", "REC-D", 0.0, true, true),
        ("REC-C", "REC-D", 120.0, false, false),
        ("REC-A", "REC-E", 48.0, true, false),
    ];
    assert_eq!(found.len(), expected.len(), "{found:?}");
    for ((from, to, gap, driving, critical), want) in found.iter().zip(expected) {
        assert_eq!(
            (from.as_str(), to.as_str(), *driving, *critical),
            (want.0, want.1, want.3, want.4)
        );
        assert!((gap - want.2).abs() < 1e-9, "{from}→{to} gap {gap}");
    }
}
