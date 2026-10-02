//! Time passing changes answers with no new revision; the boundary says when, so no client
//! rebuilds readiness from domain rules.

use super::*;
use chrono::TimeDelta;
use dpm_model::{ActivityKind, Observation, RunId};

fn explain_blocking(app: &mut Application, key: &str) -> View {
    view_blocking(app, json!({"query": "explain", "key": key}))
}

fn ready(view: &View) -> bool {
    view.envelope.data["ready"].as_bool().expect("ready")
}

/// A started TEST-A whose half-hour lag gates TEST-B, and the instant the task started.
fn lagged_blocking() -> (Application, DateTime<Utc>) {
    let mut app = Application::in_memory_blocking(&lagged_plan()).expect("app");
    let task = task_blocking(&app);
    commit_blocking(&mut app, &worker(), Command::Claim { work: task });
    commit_blocking(
        &mut app,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    let started = app
        .show_blocking("TEST-A")
        .expect("show")
        .data
        .work
        .execution
        .events
        .started_at
        .expect("started");
    (app, started)
}

#[test]
fn a_known_lag_names_its_opening_instant_and_the_view_flips_exactly_there() {
    let (mut app, started) = lagged_blocking();
    let opens = started + TimeDelta::seconds(1800);
    let feed = app.watch_commits();

    at(&mut app, started + TimeDelta::seconds(1799));
    let waiting = explain_blocking(&mut app, "TEST-B");
    assert!(!ready(&waiting));
    assert_eq!(
        waiting.refresh_at,
        Some(opens),
        "a pending elapsed lag is announced, not left to a fallback poll"
    );

    // One millisecond before the boundary is still waiting and still names the boundary.
    at(&mut app, opens - TimeDelta::milliseconds(1));
    let before = explain_blocking(&mut app, "TEST-B");
    assert!(!ready(&before));
    assert_eq!(before.refresh_at, Some(opens));

    // A consumer follows the instruction it was given: re-query at `refresh_at`, nothing else.
    at(&mut app, waiting.refresh_at.expect("announced"));
    let released = explain_blocking(&mut app, "TEST-B");
    assert!(ready(&released), "ready exactly at the opening instant");
    assert_eq!(released.evaluated_at, opens);
    assert_eq!(released.refresh_at, None, "nothing further is pending");

    // None of this was a project change: same watermark, no commit notification, nothing in the
    // project feed.
    assert_eq!(waiting.basis, released.basis);
    assert_eq!(before.basis, released.basis);
    assert!(feed.try_recv().is_err());
    let cursors = Cursors {
        project: Some(ProjectCursor {
            lineage_id: super::harness::project_basis(&released).lineage_id,
            after_sequence: super::harness::project_basis(&released).history_head,
        }),
        ..Cursors::default()
    };
    let idle = changes_blocking(&mut app, cursors, None);
    assert!(idle.project.expect("project").entries.is_empty());
    assert_eq!(Some(idle.watermark.project), released.basis.project);
    assert_eq!(idle.evaluated_at, opens);
}

#[test]
fn readiness_in_every_answer_comes_from_the_shared_evaluator_at_the_stated_instant() {
    let (mut app, started) = lagged_blocking();
    let opens = started + TimeDelta::seconds(1800);
    for (clock, expect) in [
        (opens - TimeDelta::seconds(1), false),
        (opens, true),
        (opens + TimeDelta::seconds(1), true),
    ] {
        at(&mut app, clock);
        let native = explain_blocking(&mut app, "TEST-B");
        let shared = app.explain_blocking("TEST-B").expect("explain").data.ready;
        assert_eq!(ready(&native), shared, "{clock}");
        assert_eq!(shared, expect, "{clock}");
    }
}

#[test]
fn a_view_that_reads_no_gates_announces_no_gate_instant() {
    let (mut app, started) = lagged_blocking();
    at(&mut app, started + TimeDelta::seconds(10));
    let history = view_blocking(
        &mut app,
        json!({"query": "history", "after_sequence": 0, "limit": 10}),
    );
    assert_eq!(history.refresh_at, None);
    let next = view_blocking(
        &mut app,
        json!({"query": "next", "capabilities": [], "probabilistic": false, "limit": 5}),
    );
    assert_eq!(
        next.refresh_at,
        Some(started + TimeDelta::seconds(1800)),
        "the ranked view depends on the same gate"
    );
}

#[test]
fn run_staleness_is_announced_as_a_deadline_and_a_heartbeat_moves_only_the_activity_feed() {
    let (mut app, started) = lagged_blocking();
    let run = app
        .start_run_blocking(crate::RunStartRequest {
            actor: worker(),
            work_key: "TEST-A".into(),
            executor: None,
            run_id: Some(RunId::new()),
            parent: None,
            session: None,
            observation: Observation::ReportedOnly,
            sources: Vec::new(),
            observed_at: None,
            base_lineage: None,
        })
        .expect("run")
        .data
        .run;
    let received = run.last_receipt_at;
    let stale = received + TimeDelta::seconds(300);
    at(&mut app, received);
    let fresh = view_blocking(&mut app, json!({"query": "runs"}));
    assert_eq!(
        fresh.refresh_at,
        Some(stale),
        "staleness is a deadline, not a poll"
    );
    let one = fresh.envelope.data["runs"][0].clone();
    assert_eq!(one["status"], "working");

    // The run view is stale from its deadline with no report and no project change.
    at(&mut app, stale);
    let expired = view_blocking(&mut app, json!({"query": "runs"}));
    assert_eq!(expired.envelope.data["runs"][0]["status"], "stale");
    assert_eq!(expired.refresh_at, None);
    assert_eq!(fresh.basis, expired.basis);

    // A heartbeat extends the deadline and appends to the activity feed only.
    at(&mut app, received + TimeDelta::seconds(100));
    app.record_run_activity_blocking(crate::RunActivityRequest {
        actor: worker(),
        entries: vec![dpm_model::ActivityInput {
            run: run.run.id,
            source_sequence: 1,
            kind: ActivityKind::Heartbeat,
            text: None,
            observed_at: None,
        }],
        base_lineage: None,
    })
    .expect("heartbeat");
    let again = view_blocking(&mut app, json!({"query": "run", "id": run.run.id}));
    assert_eq!(super::harness::run_basis(&again).activity_head, 1);
    assert_eq!(
        super::harness::run_basis(&again).lifecycle_head,
        super::harness::run_basis(&fresh).lifecycle_head
    );
    assert_eq!(again.basis.project, fresh.basis.project);
    assert!(again.refresh_at.is_some_and(|at| at > stale));

    // A gate-reading answer keeps announcing its own gate, whatever the runs are doing.
    let status = view_blocking(&mut app, json!({"query": "status", "probabilistic": false}));
    assert_eq!(status.refresh_at, Some(started + TimeDelta::seconds(1800)));
}
