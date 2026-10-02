//! The fixtures the worked sequences share: runs and their records, a second connection on one
//! store, and the bases an answer carries.

use super::*;
use dpm_model::{ActivityKind, Observation, RunId, RunState};

pub(super) fn start_run_blocking(app: &mut Application) -> RunId {
    app.start_run_blocking(crate::RunStartRequest {
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
    .run
    .run
    .id
}

pub(super) fn activity_blocking(app: &mut Application, run: RunId, source_sequence: u64) {
    app.record_run_activity_blocking(crate::RunActivityRequest {
        actor: worker(),
        entries: vec![dpm_model::ActivityInput {
            run,
            source_sequence,
            kind: ActivityKind::Progress,
            text: None,
            observed_at: None,
        }],
        base_lineage: None,
    })
    .expect("activity");
}

pub(super) fn report_blocking(app: &mut Application, run: RunId, state: RunState) {
    app.report_run_blocking(crate::RunReportRequest {
        actor: worker(),
        run,
        state,
        event_id: None,
        detail: None,
        observed_at: None,
        base_lineage: None,
    })
    .expect("report");
}

/// A live store with TEST-A claimed (operation 1) and a second connection on the same file, as
/// another process would hold.
pub(super) fn shared_blocking(directory: &Path) -> (Application, Application) {
    let mut app = live_blocking(directory);
    let task = task_blocking(&app);
    commit_blocking(&mut app, &worker(), Command::Claim { work: task });
    let other = Application::open_blocking(directory.join("state.sqlite")).expect("second");
    (app, other)
}

/// The basis a view carries for the project, which every answer has.
pub(super) fn project_basis(view: &View) -> ProjectWatermark {
    view.basis
        .project
        .expect("every answer is anchored to the project")
}

/// The basis a run query carries for the run store.
pub(super) fn run_basis(view: &View) -> dpm_model::RunFeedHeads {
    view.basis
        .runs
        .expect("a run query is anchored to the run store")
}
