#![allow(clippy::expect_used)]
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
        })
        .expect("status");
    assert_eq!(status.data["total_work"], 7);
    assert!(app.ensure_writable().is_err(), "a preview never writes");
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
