use super::*;
use crate::CommandRequest;
use chrono::Duration;
use dpm_engine::Command;
use dpm_model::{ActorId, WorkStatus};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

/// A workspace whose TEST-A was claimed, started, submitted and verified, with each commit time.
fn verified() -> (Application, Vec<DateTime<Utc>>) {
    let mut app = Application::in_memory_blocking(&fixture()).expect("app");
    let work = app.work_id_blocking("TEST-A").expect("work");
    let steps = [
        ("worker", Command::claim(work)),
        (
            "worker",
            Command::Start {
                work,
                occurred_at: None,
            },
        ),
        (
            "worker",
            Command::Submit {
                work,
                note: Some("done".into()),
                occurred_at: None,
            },
        ),
        (
            "reviewer",
            Command::Verify {
                work,
                note: Some("checked".into()),
                occurred_at: None,
            },
        ),
    ];
    let mut times = Vec::new();
    for (revision, (actor, command)) in steps.into_iter().enumerate() {
        let actor = if actor == "reviewer" {
            ActorId::human(actor)
        } else {
            ActorId::agent(actor)
        };
        let committed = app
            .execute_blocking(CommandRequest {
                actor,
                base_revision: revision as u64,
                base_lineage: None,
                operation_id: None,
                command,
            })
            .expect("mutation");
        times.push(committed.operation.timestamp);
    }
    (app, times)
}

fn status_of(app: &Application) -> (u64, WorkStatus, usize) {
    let shown = app.show_blocking("TEST-A").expect("show");
    let status = app.status_blocking(false, None).expect("status");
    (
        shown.revision,
        shown.data.work.execution.status,
        status.data.summary.complete,
    )
}

#[test]
fn a_pinned_clock_observes_the_workspace_as_it_stood_at_that_instant() {
    let (app, times) = verified();
    let app = app.with_query_clock(QueryClock::Pinned(times[0] - Duration::hours(1)));
    assert_eq!(
        status_of(&app),
        (4, WorkStatus::Planned, 0),
        "before the claim"
    );
    let app = app.with_query_clock(QueryClock::Pinned(times[1]));
    assert_eq!(
        status_of(&app),
        (4, WorkStatus::InProgress, 0),
        "after the start"
    );
    let app = app.with_query_clock(QueryClock::Pinned(times[3]));
    assert_eq!(
        status_of(&app),
        (4, WorkStatus::Verified, 1),
        "after the verification"
    );
}

#[test]
fn a_look_back_calibrates_and_forecasts_from_what_had_occurred_by_then() {
    let (app, times) = verified();
    let app = app.with_query_clock(QueryClock::Pinned(times[1]));
    let calibration = app.calibration_blocking().expect("calibration");
    assert_eq!(calibration.data.history.operations, 2, "claim and start");
    let schedule = app.schedule_blocking(false).expect("schedule");
    assert_eq!(schedule.data.evaluated_at, times[1]);
    let started = schedule
        .data
        .work
        .iter()
        .find(|row| row.key.to_string() == "TEST-A")
        .expect("row");
    assert!(
        started.span.as_ref().is_some_and(|s| s.finish_hours > 0.0),
        "started work still has remaining time at that instant: {started:?}"
    );
}

#[test]
fn only_a_callers_pinned_reading_looks_back() {
    let (app, times) = verified();
    let app = app.with_query_clock(QueryClock::Fixed(times[0] - Duration::hours(1)));
    assert_eq!(
        status_of(&app).1,
        WorkStatus::Verified,
        "an internal reading evaluates the snapshot"
    );
}

#[test]
fn events_an_imported_plan_carries_look_back_to_when_they_occurred() {
    let mut plan = fixture();
    let started = Utc::now() - Duration::days(365 * 80);
    let work = plan.find_work_by_key("TEST-A").expect("work").id;
    let item = plan.work_items.get_mut(&work).expect("item");
    item.execution.status = WorkStatus::InProgress;
    item.execution.owner = Some(ActorId::agent("worker"));
    item.execution.events.started_at = Some(started);
    let app = Application::in_memory_blocking(&plan)
        .expect("app")
        .with_query_clock(QueryClock::Pinned(started - Duration::minutes(1)));
    assert_eq!(status_of(&app).1, WorkStatus::Planned, "before the start");
    let app = app.with_query_clock(QueryClock::Pinned(started));
    assert_eq!(status_of(&app).1, WorkStatus::InProgress, "at the start");
}
