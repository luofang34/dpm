//! The run store against real files: idempotent writes, bounded activity and refusals that leave
//! the file as it was.

use super::*;
use chrono::{TimeDelta, Utc};
use dpm_engine::{Command, OperationFacts, RunError, apply_command, observe_run_start};
use dpm_model::{
    ActivityInput, ActivityKind, ActorId, OperationId, Plan, RunEventId, RunId, RunRecord,
    RunSession, RunStart, RunState, RunTransition, WorkItemId,
};

mod activity;
mod foreign;
mod pairing;
mod recovery;
mod restores;
mod retries;

pub(super) fn worker() -> ActorId {
    ActorId::agent("worker")
}

/// The fixture plan, with its first task not yet claimed.
pub(super) fn unclaimed() -> (Plan, WorkItemId) {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    (plan, work)
}

/// A fixture plan whose first task is claimed by the worker.
pub(super) fn claimed() -> (Plan, WorkItemId) {
    let (mut plan, work) = unclaimed();
    apply_command(
        &mut plan,
        worker(),
        Command::Claim { work },
        Utc::now(),
        OperationId::new(),
    )
    .expect("claim");
    (plan, work)
}

pub(super) fn request(work: WorkItemId) -> RunStart {
    RunStart {
        id: RunId::new(),
        work,
        executor: worker(),
        parent: None,
        session: None,
        observation: dpm_model::Observation::ReportedOnly,
        sources: Vec::new(),
        observed_at: None,
    }
}

pub(super) fn record(plan: &Plan, lineage: LineageId, request: &RunStart) -> RunRecord {
    observe_run_start(plan, lineage, &worker(), request, Utc::now()).expect("run")
}

/// A run store bound to the fixture workspace and a fresh lineage.
pub(super) struct Fixture {
    pub(super) plan: Plan,
    pub(super) work: WorkItemId,
    pub(super) lineage: LineageId,
    pub(super) store: RunStore,
}

pub(super) fn fixture() -> Fixture {
    let (plan, work) = claimed();
    let lineage = LineageId::new();
    let store = RunStore::in_memory_blocking(plan.workspace.id, lineage).expect("store");
    Fixture {
        plan,
        work,
        lineage,
        store,
    }
}

impl Fixture {
    pub(super) fn start(&mut self) -> RunRecord {
        let record = record(&self.plan, self.lineage, &request(self.work));
        self.store
            .start_blocking(&record, self.lineage)
            .expect("start");
        record
    }

    pub(super) fn transition(
        &mut self,
        run: RunId,
        to: RunState,
    ) -> Result<Written<dpm_model::LifecycleEntry>, RunStoreError> {
        let transition = RunTransition {
            id: RunEventId::new(),
            run,
            to,
            detail: None,
            observed_at: None,
        };
        self.store
            .transition_blocking(&transition, &worker(), Utc::now(), self.lineage)
    }

    pub(super) fn record_activity(
        &mut self,
        run: RunId,
        source_sequence: u64,
        kind: ActivityKind,
    ) -> Result<Vec<Written<dpm_model::ActivityEntry>>, RunStoreError> {
        let input = ActivityInput {
            run,
            source_sequence,
            kind,
            text: None,
            observed_at: None,
        };
        self.store
            .append_activity_blocking(&[input], &worker(), Utc::now(), self.lineage)
    }
}

#[test]
fn a_task_can_have_several_runs_and_each_keeps_what_it_observed() {
    let mut fixture = fixture();
    let first = fixture.start();
    fixture
        .transition(first.id, RunState::Failed)
        .expect("fail");
    let second = fixture.start();
    let runs = fixture
        .store
        .snapshots_blocking(Some(fixture.work), 10)
        .expect("runs");
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].record.id, second.id, "newest first");
    assert_eq!(runs[1].record.id, first.id);
    assert_eq!(
        (runs[0].last.state, runs[1].last.state),
        (RunState::Working, RunState::Failed)
    );
    for run in &runs {
        assert_eq!(run.record.work, fixture.work);
        assert_eq!(
            run.record.contract.contract,
            fixture.plan.work_items[&fixture.work].contract
        );
        assert_eq!(run.record.contract.revision, fixture.plan.revision);
    }
    let other = fixture
        .store
        .snapshots_blocking(Some(WorkItemId::new()), 10)
        .expect("runs");
    assert!(other.is_empty());
}

#[test]
fn a_resent_start_returns_the_recorded_run_and_a_changed_one_is_refused() {
    let mut fixture = fixture();
    let record = fixture.start();
    let again = fixture
        .store
        .start_blocking(&record, fixture.lineage)
        .expect("resend");
    assert!(again.replayed);
    assert_eq!(again.value.sequence, 1);
    let mut changed = record.clone();
    changed.session = Some(RunSession {
        provider: "codex".into(),
        session: "other".into(),
        turn: None,
    });
    assert!(matches!(
        fixture.store.start_blocking(&changed, fixture.lineage),
        Err(RunStoreError::DuplicateRun { .. })
    ));
    let mut other_writer = record.clone();
    other_writer.recorded_by = ActorId::service("host");
    assert!(matches!(
        fixture.store.start_blocking(&other_writer, fixture.lineage),
        Err(RunStoreError::DuplicateRun { .. })
    ));
    let page = fixture
        .store
        .lifecycle_page_blocking(0, 100, None)
        .expect("page");
    assert_eq!(page.entries.len(), 1, "the resends recorded nothing");
    assert_eq!(page.head_sequence, 1);
}

#[test]
fn a_start_names_a_known_parent_or_is_refused() {
    let mut fixture = fixture();
    let parent = fixture.start();
    let mut child = request(fixture.work);
    child.parent = Some(parent.id);
    let child = record(&fixture.plan, fixture.lineage, &child);
    fixture
        .store
        .start_blocking(&child, fixture.lineage)
        .expect("child");
    let mut orphan = request(fixture.work);
    orphan.parent = Some(RunId::new());
    let orphan = record(&fixture.plan, fixture.lineage, &orphan);
    assert!(matches!(
        fixture.store.start_blocking(&orphan, fixture.lineage),
        Err(RunStoreError::UnknownParent(_))
    ));
}

#[test]
fn lifecycle_follows_the_state_machine_and_a_terminal_state_is_final() {
    let mut fixture = fixture();
    let run = fixture.start().id;
    for state in [RunState::Waiting, RunState::Working, RunState::Completed] {
        fixture.transition(run, state).expect("legal");
    }
    for state in [RunState::Working, RunState::Failed, RunState::Completed] {
        assert!(matches!(
            fixture.transition(run, state),
            Err(RunStoreError::Run(RunError::Terminal { .. }))
        ));
    }
    let second = fixture.start().id;
    assert!(matches!(
        fixture.transition(second, RunState::Working),
        Err(RunStoreError::Run(RunError::InvalidTransition { .. }))
    ));
    assert!(matches!(
        fixture.transition(RunId::new(), RunState::Waiting),
        Err(RunStoreError::UnknownRun(_))
    ));
    let snapshot = fixture
        .store
        .snapshot_blocking(run)
        .expect("read")
        .expect("run");
    assert_eq!(snapshot.last.state, RunState::Completed);
}

#[test]
fn a_resent_transition_is_answered_and_a_changed_one_is_refused() {
    let mut fixture = fixture();
    let run = fixture.start().id;
    let transition = RunTransition {
        id: RunEventId::new(),
        run,
        to: RunState::Waiting,
        detail: Some("needs input".into()),
        observed_at: None,
    };
    let first = fixture
        .store
        .transition_blocking(&transition, &worker(), Utc::now(), fixture.lineage)
        .expect("first");
    assert!(!first.replayed);
    // Even after the run moved on, the resend carries the same identity and is answered.
    fixture.transition(run, RunState::Working).expect("resume");
    let again = fixture
        .store
        .transition_blocking(&transition, &worker(), Utc::now(), fixture.lineage)
        .expect("resend");
    assert!(again.replayed);
    assert_eq!(again.value, first.value);
    let changed = RunTransition {
        to: RunState::Failed,
        ..transition.clone()
    };
    assert!(matches!(
        fixture
            .store
            .transition_blocking(&changed, &worker(), Utc::now(), fixture.lineage),
        Err(RunStoreError::DuplicateEvent { .. })
    ));
    assert!(matches!(
        fixture.store.transition_blocking(
            &transition,
            &ActorId::human("lead"),
            Utc::now(),
            fixture.lineage
        ),
        Err(RunStoreError::DuplicateEvent { .. })
    ));
    let page = fixture
        .store
        .lifecycle_page_blocking(0, 100, Some(run))
        .expect("page");
    assert_eq!(page.entries.len(), 3, "start, waiting, working");
}

#[test]
fn only_the_executor_a_human_or_a_service_may_report_on_a_run() {
    let mut fixture = fixture();
    let run = fixture.start().id;
    let transition = |to| RunTransition {
        id: RunEventId::new(),
        run,
        to,
        detail: None,
        observed_at: None,
    };
    assert!(matches!(
        fixture.store.transition_blocking(
            &transition(RunState::Failed),
            &ActorId::agent("rival"),
            Utc::now(),
            fixture.lineage
        ),
        Err(RunStoreError::Run(RunError::ActorNotAllowed { .. }))
    ));
    fixture
        .store
        .transition_blocking(
            &transition(RunState::Interrupted),
            &ActorId::human("lead"),
            Utc::now(),
            fixture.lineage,
        )
        .expect("an operator may record an interruption");
}
