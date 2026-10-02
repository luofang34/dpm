//! Freshness, feeds, links and refusals.

use super::*;

#[test]
fn silence_makes_a_run_stale_on_the_query_clock_and_never_finished() {
    let mut app = started();
    let run = start(&mut app);
    let began = app.run_blocking(run).expect("run").data.run.started_at;
    for (seconds, status) in [
        (-3600, ObservedStatus::Working),
        (299, ObservedStatus::Working),
        (300, ObservedStatus::Stale),
        (86_400 * 30, ObservedStatus::Stale),
    ] {
        at(&mut app, began + TimeDelta::seconds(seconds));
        let view = app.run_blocking(run).expect("run").data;
        assert_eq!(view.status, status, "{seconds} s");
        assert_eq!(
            view.state,
            RunState::Working,
            "silence is not a lifecycle fact"
        );
        assert_eq!(view.evaluated_at, began + TimeDelta::seconds(seconds));
    }
    at(&mut app, began + TimeDelta::seconds(10));
    assert_eq!(
        app.run_blocking(run).expect("run").data.stale_at,
        Some(began + TimeDelta::seconds(300))
    );
}

#[test]
fn a_reported_future_time_cannot_keep_a_dead_run_fresh() {
    let mut app = started();
    let run = start(&mut app);
    let began = app.run_blocking(run).expect("run").data.run.started_at;
    let mut boast = activity(run, 1, ActivityKind::Heartbeat);
    boast.entries[0].observed_at = Some(began + TimeDelta::days(3650));
    let receipt = app.record_run_activity_blocking(boast).expect("heartbeat");
    let recorded_at = receipt.data.entries[0].entry.record.recorded_at;
    assert!(
        recorded_at < began + TimeDelta::days(1),
        "DPM stamps its own receipt time"
    );
    at(&mut app, recorded_at + TimeDelta::seconds(299));
    assert_eq!(
        app.run_blocking(run).expect("run").data.status,
        ObservedStatus::Working
    );
    at(&mut app, recorded_at + TimeDelta::seconds(301));
    let view = app.run_blocking(run).expect("run").data;
    assert_eq!(view.status, ObservedStatus::Stale);
    assert_eq!(view.last_receipt_at, recorded_at);
}

#[test]
fn a_heartbeat_refreshes_a_stale_run_and_an_operator_can_record_its_interruption() {
    let mut app = started();
    let run = start(&mut app);
    let began = app.run_blocking(run).expect("run").data.run.started_at;
    at(&mut app, began + TimeDelta::hours(2));
    assert_eq!(
        app.run_blocking(run).expect("run").data.status,
        ObservedStatus::Stale
    );
    // Silence is not repaired by an agent that is not the executor.
    let mut stranger = report(run, RunState::Interrupted);
    stranger.actor = ActorId::agent("stranger");
    assert_eq!(
        code(app.report_run_blocking(stranger).expect_err("refused")),
        "invalid_command"
    );
    let mut operator = report(run, RunState::Interrupted);
    operator.actor = ActorId::human("lead");
    operator.detail = Some("host rebooted".into());
    let view = app
        .report_run_blocking(operator)
        .expect("interrupt")
        .data
        .run;
    assert_eq!(view.status, ObservedStatus::Interrupted);
    let item = &app.plan_blocking().expect("plan").work_items[&work(&app)];
    assert_eq!(
        item.execution.owner,
        Some(worker()),
        "recovering a run releases nothing"
    );
}

#[test]
fn a_run_whose_task_changed_hands_is_reported_orphaned_and_nothing_moves() {
    let mut app = started();
    let run = start(&mut app);
    assert!(app.run_blocking(run).expect("run").data.orphan.is_none());
    let id = work(&app);
    project(
        &mut app,
        &ActorId::human("lead"),
        Command::Handoff {
            work: id,
            from: worker(),
            to: ActorId::agent("heir"),
            reason: "rebalance".into(),
        },
    );
    let view = app.run_blocking(run).expect("run").data;
    assert_eq!(
        view.orphan,
        Some(dpm_model::Orphan::OwnerChanged {
            owner: Some(ActorId::agent("heir"))
        })
    );
    assert_eq!(
        view.state,
        RunState::Working,
        "the run itself is not rewritten"
    );
    let item = &app.plan_blocking().expect("plan").work_items[&id];
    assert_eq!(item.execution.owner, Some(ActorId::agent("heir")));
}

#[test]
fn activity_and_lifecycle_have_independent_cursors_and_retention_reports_its_gap() {
    let mut app = started();
    app.set_run_retention(3);
    let run = start(&mut app);
    for index in 1..=5 {
        app.record_run_activity_blocking(activity(run, index, ActivityKind::Progress))
            .expect("activity");
    }
    app.report_run_blocking(report(run, RunState::Waiting))
        .expect("waiting");
    let page = app
        .run_activity_blocking(0, 100, None)
        .expect("activity")
        .data;
    let gap = page.gap.expect("the cursor outran retention");
    assert_eq!((gap.requested_after, gap.resumes_at, gap.lost), (0, 3, 2));
    assert_eq!(page.entries.len(), 3);
    assert_eq!(page.head_sequence, 5);
    let caught_up = app
        .run_activity_blocking(page.next_after_sequence, 100, None)
        .expect("tail")
        .data;
    assert!(caught_up.entries.is_empty() && caught_up.gap.is_none());
    // The lifecycle cursor is unaffected by retention and by activity.
    let lifecycle = app
        .run_lifecycle_blocking(0, 100, None)
        .expect("lifecycle")
        .data;
    assert_eq!(lifecycle.entries.len(), 2);
    assert_eq!(lifecycle.head_sequence, 2);
    let view = app.run_blocking(run).expect("run").data;
    assert_eq!((view.activity.recorded, view.activity.retained), (5, 3));
    assert_eq!(view.state, RunState::Waiting);
}

#[test]
fn a_preview_refuses_run_writes_and_answers_reads_empty() {
    let mut app = Application::preview(fixture_plan()).expect("preview");
    for refusal in [
        app.start_run_blocking(start_request()).map(|_| ()),
        app.report_run_blocking(report(RunId::new(), RunState::Failed))
            .map(|_| ()),
        app.record_run_activity_blocking(activity(RunId::new(), 1, ActivityKind::Heartbeat))
            .map(|_| ()),
    ] {
        assert_eq!(code(refusal.expect_err("refused")), "read_only_project");
    }
    assert!(
        app.runs_blocking(&RunQuery {
            key: None,
            limit: 10
        })
        .expect("runs")
        .data
        .runs
        .is_empty()
    );
    assert_eq!(
        app.run_lifecycle_blocking(0, 10, None)
            .expect("feed")
            .data
            .head_sequence,
        0
    );
    assert_eq!(
        app.run_activity_blocking(7, 10, None)
            .expect("feed")
            .data
            .next_after_sequence,
        7
    );
    assert_eq!(
        code(app.run_blocking(RunId::new()).expect_err("unknown")),
        "not_found"
    );
}

#[test]
fn the_json_boundary_returns_the_same_envelope_the_typed_calls_serialize() {
    let mut app = started();
    let request = start_request();
    let response = app
        .execute_run_blocking(RunCommand::Start(request.clone()))
        .expect("start");
    assert_eq!(response.api_version, crate::API_VERSION);
    assert_eq!(
        response.revision,
        app.revision_blocking().expect("revision").revision
    );
    assert_eq!(response.data["replayed"], false);
    let run = request.run_id.expect("id");
    let query = app.query_blocking(Query::Run { id: run }).expect("query");
    assert_eq!(query.data["run"]["id"], serde_json::json!(run));
    assert_eq!(query.data["status"], "working");
    let listed = app
        .query_blocking(Query::Runs {
            key: Some("TEST-A".into()),
            limit: 10,
        })
        .expect("list");
    assert_eq!(listed.data["runs"].as_array().map(Vec::len), Some(1));
    let page = app
        .query_blocking(Query::RunLifecycle {
            after_sequence: 0,
            limit: 10,
            run: None,
        })
        .expect("feed");
    assert_eq!(page.data["head_sequence"], 1);
    let parsed: Query =
        serde_json::from_value(serde_json::json!({"query": "run_activity", "run": run}))
            .expect("defaults");
    assert!(matches!(
        parsed,
        Query::RunActivity {
            after_sequence: 0,
            limit: 100,
            ..
        }
    ));
}

#[test]
fn an_operation_links_to_a_run_only_when_it_is_provably_the_runs_own() {
    let mut app = started();
    let work = work(&app);
    let run = start(&mut app);
    let submit = project(
        &mut app,
        &worker(),
        Command::Submit {
            work,
            note: None,
            occurred_at: None,
        },
    );
    let link = RunLinkRequest {
        actor: worker(),
        run,
        operation: submit.operation.id,
        base_lineage: None,
    };
    let linked = app.link_run_operation_blocking(link.clone()).expect("link");
    assert!(!linked.data.replayed);
    assert_eq!(linked.data.run.operations.len(), 1);
    assert_eq!(
        linked.data.link.expect("link").operation,
        submit.operation.id
    );
    assert!(
        app.link_run_operation_blocking(link.clone())
            .expect("retry")
            .data
            .replayed
    );
    // The independent review is someone else's operation and cannot be attributed to the run.
    let verify = project(
        &mut app,
        &ActorId::human("reviewer"),
        Command::Verify {
            work,
            note: None,
            occurred_at: None,
        },
    );
    let refused = app
        .link_run_operation_blocking(RunLinkRequest {
            operation: verify.operation.id,
            ..link.clone()
        })
        .expect_err("not the executor's");
    assert_eq!(code(refused), "run_link_refused");
    let unknown = app
        .link_run_operation_blocking(RunLinkRequest {
            operation: dpm_model::OperationId::new(),
            ..link
        })
        .expect_err("unknown");
    assert_eq!(code(unknown), "not_found");
    let operations = app.run_blocking(run).expect("run").data.operations;
    assert_eq!(
        operations.len(),
        1,
        "only the provable attribution was recorded"
    );
}

#[test]
fn run_writes_for_another_lineage_are_refused_and_record_nothing() {
    let mut app = started();
    let mut request = start_request();
    request.base_lineage = Some(dpm_model::LineageId::new());
    assert_eq!(
        code(app.start_run_blocking(request).expect_err("refused")),
        "lineage_mismatch"
    );
    assert!(
        app.runs_blocking(&RunQuery {
            key: None,
            limit: 10
        })
        .expect("runs")
        .data
        .runs
        .is_empty()
    );
}

#[test]
fn a_run_starts_only_for_an_executor_that_owns_started_or_claimed_work() {
    let mut app = Application::in_memory_blocking(&fixture_plan()).expect("app");
    assert_eq!(
        code(
            app.start_run_blocking(start_request())
                .expect_err("unclaimed")
        ),
        "invalid_command"
    );
    let id = work(&app);
    project(&mut app, &worker(), Command::Claim { work: id });
    let mut other = start_request();
    other.actor = ActorId::human("lead");
    other.executor = Some(ActorId::agent("someone-else"));
    assert_eq!(
        code(app.start_run_blocking(other).expect_err("not the owner")),
        "invalid_command"
    );
    let mut unknown = start_request();
    unknown.work_key = "NOPE-1".into();
    assert_eq!(
        code(app.start_run_blocking(unknown).expect_err("unknown")),
        "not_found"
    );
    let mut managed = start_request();
    managed.observation = Observation::Managed;
    assert_eq!(
        code(
            app.start_run_blocking(managed.clone())
                .expect_err("agent cannot")
        ),
        "invalid_command"
    );
    managed.actor = ActorId::service("host");
    managed.executor = Some(worker());
    app.start_run_blocking(managed)
        .expect("a service may record a managed run");
}
