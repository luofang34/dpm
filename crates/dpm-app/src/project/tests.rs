use super::*;
use crate::Query;

#[cfg(feature = "sqlite")]
mod store;

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("synthetic plan")
}

/// A client build without SQLite still opens a preview and answers every query from it.
#[test]
fn a_preview_answers_queries_in_every_feature_set() {
    let app = Application::preview(fixture()).expect("preview");
    let status = app
        .query_blocking(Query::Status {
            probabilistic: false,
            calibrated: false,
        })
        .expect("status");
    assert_eq!(status.data["total_work"], 7);
    let calibration = app.query_blocking(Query::Calibration).expect("calibration");
    assert_eq!(calibration.data["history"]["operations"], 0);
    assert_eq!(
        calibration.data["estimates"]["samples"],
        serde_json::json!([])
    );
    assert!(app.ensure_writable().is_err(), "a preview never writes");
}

/// Runs are observations beside the plan; a build or source without a run store answers run reads
/// empty and refuses run writes the way it refuses project mutations.
#[test]
fn run_queries_are_empty_and_run_writes_are_refused_in_every_feature_set() {
    let mut app = Application::preview(fixture()).expect("preview");
    let runs = app
        .query_blocking(Query::Runs {
            key: None,
            limit: 10,
        })
        .expect("runs");
    assert_eq!(runs.data["runs"], serde_json::json!([]));
    let lifecycle = app
        .query_blocking(Query::RunLifecycle {
            after_sequence: 4,
            limit: 10,
            run: None,
        })
        .expect("lifecycle feed");
    assert_eq!(lifecycle.data["head_sequence"], 0);
    assert_eq!(lifecycle.data["next_after_sequence"], 4);
    let activity = app
        .query_blocking(Query::RunActivity {
            after_sequence: 0,
            limit: 10,
            run: None,
        })
        .expect("activity feed");
    assert_eq!(activity.data["entries"], serde_json::json!([]));
    let refused = app
        .execute_run_blocking(crate::RunCommand::Report(crate::RunReportRequest {
            actor: dpm_model::ActorId::agent("worker"),
            run: dpm_model::RunId::new(),
            state: dpm_model::RunState::Failed,
            event_id: None,
            detail: None,
            observed_at: None,
            base_lineage: None,
        }))
        .expect_err("no run writes");
    assert_eq!(refused.code(), "read_only_project");
}

/// Without SQLite a store path is refused with the stable `unsupported` code, not a panic or an
/// opaque storage error, so a client can tell the user which capability its build lacks.
#[cfg(not(feature = "sqlite"))]
#[test]
fn stores_need_the_sqlite_feature() {
    let temp = tempfile::TempDir::new().expect("temp");
    let error = open_workspace_blocking(temp.path(), None, Some(Path::new("state.sqlite")))
        .err()
        .expect("refused");
    assert_eq!(error.code(), "unsupported");
    assert!(matches!(error, AppError::Unsupported { feature: "sqlite" }));
}
