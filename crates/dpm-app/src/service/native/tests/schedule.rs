//! The schedule query through the native boundary: the same envelope as the shared query, its own
//! basis, and no write from a live store, a preview or an archive.

use super::{at, attach_blocking, fixture_plan, live_blocking, view_blocking, worker};
use crate::{Application, Envelope, Query};
use chrono::{DateTime, Utc};
use dpm_engine::Command;
use serde_json::{Value, json};

fn pinned() -> DateTime<Utc> {
    "2026-10-02T12:00:00Z".parse().expect("time")
}

fn schedule(probabilistic: bool) -> Value {
    json!({"query": "schedule", "probabilistic": probabilistic})
}

/// The native result equals the shared query's envelope, and its basis names the pinned clock.
fn assert_parity(app: &mut Application, probabilistic: bool) -> Value {
    let native = view_blocking(app, schedule(probabilistic));
    let query: Query = serde_json::from_value(schedule(probabilistic)).expect("query");
    let shared = Envelope::from(app.query_blocking(query).expect("shared query"));
    assert_eq!(
        serde_json::to_value(&native.envelope).expect("json"),
        serde_json::to_value(&shared).expect("json")
    );
    assert_eq!(native.evaluated_at, pinned());
    assert_eq!(
        native.envelope.revision,
        Some(app.revision_blocking().expect("revision").revision)
    );
    assert!(native.basis.project.is_some());
    native.envelope.data
}

#[test]
fn a_live_store_answers_the_shared_schedule_with_its_own_basis() {
    let directory = tempfile::tempdir().expect("directory");
    let mut app = live_blocking(directory.path());
    let task = app.work_id_blocking("TEST-A").expect("task");
    super::commit_blocking(&mut app, &worker(), Command::Claim { work: task });
    at(&mut app, pinned());
    let plain = assert_parity(&mut app, false);
    assert!(plain["uncertainty"].is_null());
    assert!(
        plain["work"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
    );
    let sampled = assert_parity(&mut app, true);
    assert_eq!(sampled["uncertainty"]["iterations"], 2000);
    assert_eq!(
        plain["project_finish_hours"],
        sampled["project_finish_hours"]
    );
}

#[test]
fn reading_the_schedule_writes_nothing_in_a_store_a_preview_or_an_archive() {
    let directory = tempfile::tempdir().expect("directory");
    let mut app = live_blocking(directory.path());
    at(&mut app, pinned());
    let archive = directory.path().join("archive.sqlite");
    app.backup_blocking(&archive).expect("backup");
    let archived_bytes = std::fs::read(&archive).expect("archive");
    let state = |app: &Application| {
        let revision = app.revision_blocking().expect("revision");
        let history = app.history_blocking(0, 1000).expect("history");
        (revision, serde_json::to_value(history).expect("json"))
    };
    let before = state(&app);
    assert_parity(&mut app, true);
    assert_eq!(state(&app), before);

    let mut archived = Application::open_blocking(&archive).expect("open");
    at(&mut archived, pinned());
    let before = state(&archived);
    assert_parity(&mut archived, true);
    assert_eq!(state(&archived), before);
    assert_eq!(std::fs::read(&archive).expect("archive"), archived_bytes);

    let mut preview = Application::preview(fixture_plan()).expect("preview");
    at(&mut preview, pinned());
    assert_eq!(
        attach_blocking(&mut preview).source,
        crate::SourceKind::Preview
    );
    let before = state(&preview);
    assert_parity(&mut preview, true);
    assert_eq!(state(&preview), before);
}
