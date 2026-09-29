use super::*;
use crate::{CommandRequest, Query, QueryClock};
use chrono::{Duration, TimeZone};
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

/// A workspace with started work, whose forecasts depend on the clock reading.
fn started() -> Application {
    let mut app = Application::in_memory_blocking(&fixture()).expect("app");
    let work = app.work_id_blocking("TEST-A").expect("work");
    for (revision, command) in [Command::Claim { work }, Command::Start { work }]
        .into_iter()
        .enumerate()
    {
        app.execute_blocking(CommandRequest {
            actor: ActorId::agent("worker"),
            base_revision: revision as u64,
            base_lineage: None,
            operation_id: None,
            command,
        })
        .expect("mutation");
    }
    app
}

fn data(app: &Application, query: Query) -> Value {
    app.query_blocking(query).expect("query").data
}

#[test]
fn typed_results_serialize_to_the_json_adapters_return() {
    let app = started().with_query_clock(QueryClock::Fixed(pinned()));
    let plan = app.plan_blocking().expect("plan");
    let work = plan.find_work_by_key("TEST-A").expect("work").id;
    let lineage = app.lineage_blocking().expect("lineage");

    let status = app.status_blocking(false).expect("status");
    let mut expected =
        serde_json::to_value(dpm_engine::status(&plan, false, pinned()).expect("s")).expect("json");
    expected["lineage_id"] = json!(lineage);
    assert_eq!(serde_json::to_value(&status.data).expect("json"), expected);
    assert_eq!(
        data(
            &app,
            Query::Status {
                probabilistic: false
            }
        ),
        expected
    );
    assert_eq!((status.revision, status.lineage_id), (2, lineage));

    let show = app.show_blocking("TEST-A").expect("show");
    let mut expected =
        serde_json::to_value(dpm_engine::show_work(&plan, work, pinned()).expect("w"))
            .expect("json");
    let progress = dpm_engine::progress(&plan, pinned()).expect("progress");
    expected["progress"] = serde_json::to_value(progress.work[&work]).expect("json");
    assert_eq!(serde_json::to_value(&show.data).expect("json"), expected);
    let key = || "TEST-A".to_owned();
    assert_eq!(data(&app, Query::Show { key: key() }), expected);

    let explain = app.explain_blocking("TEST-A").expect("explain").data;
    assert_eq!(
        serde_json::to_value(&explain).expect("json"),
        data(&app, Query::Explain { key: key() })
    );
    let request = NextRequest {
        limit: 5,
        ..NextRequest::default()
    };
    let next = app.next_blocking(&request).expect("next").data;
    let query = Query::Next {
        capabilities: BTreeSet::new(),
        probabilistic: false,
        limit: 5,
        project_keys: BTreeSet::new(),
        asset_keys: BTreeSet::new(),
    };
    assert_eq!(
        serde_json::to_value(&next).expect("json"),
        data(&app, query)
    );
    assert_eq!(app.export_blocking().expect("export").data, plan);
}

#[test]
fn a_pinned_clock_makes_queries_reproducible_and_leaves_operation_times_real() {
    let mut app = started();
    let explain = |app: &Application| {
        data(
            app,
            Query::Explain {
                key: "TEST-A".into(),
            },
        )
    };
    // A reading just after the start, while the four-hour task still has time remaining.
    let at = Utc::now();
    app.set_query_clock(QueryClock::Fixed(at));
    assert_eq!(app.query_clock(), QueryClock::Fixed(at));
    let first = explain(&app);
    assert_eq!(explain(&app), first, "one reading, one answer");
    app.set_query_clock(QueryClock::Fixed(at + Duration::hours(1)));
    let later = explain(&app);
    let finish = |view: &Value| view["schedule"]["earliest_finish_hours"].as_f64();
    assert_eq!(
        finish(&first).zip(finish(&later)).map(|(a, b)| a - b),
        Some(1.0),
        "started work spends the hour"
    );

    let before = Utc::now();
    let work = app.work_id_blocking("TEST-A").expect("work");
    let recorded = app
        .execute_blocking(CommandRequest {
            actor: ActorId::agent("worker"),
            base_revision: 2,
            base_lineage: None,
            operation_id: None,
            command: Command::ReportProgress {
                work,
                percent: 50,
                note: None,
            },
        })
        .expect("progress");
    assert!(
        recorded.operation.timestamp >= before,
        "commit time is real"
    );
}

#[test]
fn the_revision_query_matches_the_snapshot_it_summarizes() {
    let app = started();
    let revision = app.revision_blocking().expect("revision");
    let lineage = app.lineage_blocking().expect("lineage");
    assert_eq!(
        revision,
        WorkspaceRevision {
            revision: 2,
            lineage_id: lineage
        }
    );
    let response = app.query_blocking(Query::Revision).expect("query");
    assert_eq!((response.revision, response.lineage_id), (2, lineage));
    assert_eq!(response.data, json!({"revision": 2, "lineage_id": lineage}));
    let preview = Application::preview(fixture()).expect("preview");
    assert_eq!(
        preview.revision_blocking().expect("preview revision"),
        WorkspaceRevision {
            revision: 0,
            lineage_id: None
        }
    );
}
