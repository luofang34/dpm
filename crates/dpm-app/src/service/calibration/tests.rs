use super::*;
use crate::{CommandRequest, Query, QueryClock};
use chrono::{DateTime, TimeZone, Utc};
use dpm_engine::Command;
use dpm_model::ActorId;
use serde_json::{Value, json};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn pinned() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2030, 1, 2, 3, 4, 5)
        .single()
        .expect("instant")
}

fn run(app: &mut Application, actor: ActorId, command: Command) {
    let base_revision = app.plan_blocking().expect("plan").revision;
    app.execute_blocking(CommandRequest {
        actor,
        base_revision,
        base_lineage: None,
        operation_id: None,
        command,
    })
    .expect("mutation");
}

/// TEST-A claimed and started by an agent, whose start the forecast measures from the clock.
fn started() -> Application {
    let mut app = Application::in_memory_blocking(&fixture()).expect("app");
    let work = app.work_id_blocking("TEST-A").expect("work");
    run(&mut app, ActorId::agent("worker"), Command::Claim { work });
    let start = Command::Start {
        work,
        occurred_at: None,
    };
    run(&mut app, ActorId::agent("worker"), start);
    app.with_query_clock(QueryClock::Fixed(pinned()))
}

fn data(app: &Application, query: Query) -> Value {
    app.query_blocking(query).expect("query").data
}

#[test]
fn the_default_status_is_unchanged_without_the_calibrated_option() {
    let app = started();
    let absent: Query =
        serde_json::from_value(json!({"query": "status", "probabilistic": true})).expect("query");
    let default = data(&app, absent);
    let explicit = data(
        &app,
        Query::Status {
            probabilistic: true,
            calibrated: false,
        },
    );
    let plan = app.plan_blocking().expect("plan");
    let mut engine =
        serde_json::to_value(dpm_engine::status(&plan, true, pinned()).expect("status"))
            .expect("json");
    engine["lineage_id"] = json!(app.lineage_blocking().expect("lineage"));
    assert_eq!(default.to_string(), explicit.to_string());
    assert_eq!(default, engine);
    assert!(default.get("calibration").is_none());

    // Without enough samples nothing is applied, so the calibrated forecast adds only its report.
    let mut calibrated = data(
        &app,
        Query::Status {
            probabilistic: true,
            calibrated: true,
        },
    );
    let applied = calibrated
        .as_object_mut()
        .and_then(|o| o.remove("calibration"))
        .expect("calibration");
    assert_eq!(calibrated, default);
    // Unowned work resolves to Human; TEST-A's owner is an agent.
    let factors: Vec<_> = applied["factors"]
        .as_array()
        .expect("factors")
        .iter()
        .map(|f| {
            (
                f["executor"].clone(),
                f["tasks"].clone(),
                f["applied"].clone(),
            )
        })
        .collect();
    assert_eq!(
        factors,
        [
            (json!("Human"), json!(5), json!(false)),
            (json!("Agent"), json!(1), json!(false))
        ]
    );
    assert_eq!(applied["review_delay"]["hours"], 0.0);
}

#[test]
fn calibration_reads_the_whole_log_and_names_bulk_recorded_work() {
    let mut app = started();
    let work = app.work_id_blocking("TEST-A").expect("work");
    let submit = Command::Submit {
        work,
        note: None,
        occurred_at: None,
    };
    run(&mut app, ActorId::agent("worker"), submit);
    let verify = Command::Verify {
        work,
        note: None,
        occurred_at: None,
    };
    run(&mut app, ActorId::human("reviewer"), verify);
    let response = app.query_blocking(Query::Calibration).expect("calibration");
    assert_eq!(response.revision, 4);
    let report = response.data;
    assert_eq!(report["history"]["operations"], 4);
    assert_eq!(report["estimates"]["samples"], json!([]));
    assert_eq!(
        report["estimates"]["excluded"],
        json!([{"reason": "bulk_recorded", "count": 1, "keys": ["TEST-A"]}])
    );
    assert_eq!(report["flow"]["reliability"]["total"]["verified"], 1);
    assert_eq!(report["rules"]["bulk_window_seconds"], 180);
    let typed = app.calibration_blocking().expect("typed").data;
    assert_eq!(serde_json::to_value(typed).expect("json"), report);
}

#[test]
fn next_advises_an_actor_that_holds_claims_without_changing_candidates() {
    let app = started();
    let next = |actor: Option<&str>| {
        data(
            &app,
            Query::Next {
                capabilities: Default::default(),
                probabilistic: false,
                limit: 5,
                project_keys: Default::default(),
                asset_keys: Default::default(),
                actor: actor.map(|a| a.parse().expect("actor")),
            },
        )
    };
    let anonymous = next(None);
    let holder = next(Some("agent:worker"));
    let idle = next(Some("agent:idle"));
    assert!(anonymous.get("advisories").is_none());
    assert_eq!(idle, anonymous);
    assert_eq!(holder["candidates"], anonymous["candidates"]);
    assert_eq!(holder["advisories"][0]["kind"], "holding_claims");
    assert_eq!(holder["advisories"][0]["holding"], json!(["TEST-A"]));
}

#[test]
fn a_read_retries_while_commits_land_and_then_says_the_workspace_kept_changing() {
    let mut reads = 0;
    let settled = consistent_read(|| {
        reads += 1;
        let plan = fixture();
        let revision = (reads == CONSISTENT_READS).then_some(plan.revision);
        Ok((revision.or(Some(plan.revision + 1)), plan, Vec::new()))
    });
    assert!(settled.is_ok());
    assert_eq!(reads, CONSISTENT_READS);

    let mut reads = 0;
    let busy = consistent_read(|| {
        reads += 1;
        let plan = fixture();
        Ok((None, plan, Vec::new()))
    });
    assert_eq!(reads, CONSISTENT_READS, "the retries are bounded");
    let error = busy.expect_err("never consistent");
    assert!(
        matches!(error, AppError::HistoryChanging { reads: 10 }),
        "{error:?}"
    );
    assert!(error.to_string().contains("kept changing during the read"));
    assert_eq!(error.code(), "revision_conflict");
}
