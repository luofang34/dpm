use super::*;
use crate::{Command, apply_command};
use dpm_model::{
    ActivityTally, ArtifactKind, LifecycleEvent, Observation, Plan, RunRecord, RunSnapshot,
    RunSource, RunStart, WorkItemId,
};

mod observation;
mod sources;

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn worker() -> ActorId {
    ActorId::agent("worker")
}

/// A fixture whose first task is claimed by the worker.
fn claimed() -> (Plan, WorkItemId) {
    let mut plan = fixture();
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
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

fn request(work: WorkItemId) -> RunStart {
    RunStart {
        id: RunId::new(),
        work,
        executor: worker(),
        parent: None,
        session: None,
        observation: Observation::ReportedOnly,
        sources: Vec::new(),
        observed_at: None,
    }
}

fn started(plan: &Plan, work: WorkItemId) -> (RunRecord, LineageId) {
    let lineage = LineageId::new();
    let record = observe_start(plan, lineage, &worker(), &request(work), Utc::now()).expect("run");
    (record, lineage)
}

fn snapshot(record: &RunRecord, state: RunState, receipt: DateTime<Utc>) -> RunSnapshot {
    RunSnapshot {
        last: LifecycleEvent {
            id: start_event(record.id),
            run: record.id,
            state,
            detail: None,
            observed_at: None,
            recorded_by: record.executor.clone(),
            recorded_at: receipt,
        },
        record: record.clone(),
        activity: ActivityTally {
            recorded: 0,
            retained: 0,
            source_high_water: 0,
            latest: None,
        },
        operations: Vec::new(),
    }
}

#[test]
fn a_start_captures_the_contract_revision_and_lineage_it_observed() {
    let (plan, work) = claimed();
    let (record, lineage) = started(&plan, work);
    let item = plan.work_items.get(&work).expect("work");
    assert_eq!(record.contract.contract, item.contract);
    assert_eq!(record.contract.revision, plan.revision);
    assert_eq!(record.contract.lineage_id, lineage);
    assert_eq!(record.contract.workspace_id, plan.workspace.id);
    assert_eq!(record.contract.work_key, item.key);
    assert_eq!(record.contract.prior_submissions, 0);
    assert_eq!(record.request(), request(work).clone_with_id(record.id));
}

trait WithId {
    fn clone_with_id(&self, id: RunId) -> Self;
}

impl WithId for RunStart {
    fn clone_with_id(&self, id: RunId) -> Self {
        Self { id, ..self.clone() }
    }
}

#[test]
fn a_run_starts_only_on_claimed_or_started_work_its_executor_owns() {
    let mut plan = fixture();
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    let now = Utc::now();
    let lineage = LineageId::new();
    assert!(matches!(
        observe_start(&plan, lineage, &worker(), &request(work), now),
        Err(RunError::NotExecuting { .. })
    ));
    apply_command(
        &mut plan,
        worker(),
        Command::Claim { work },
        now,
        OperationId::new(),
    )
    .expect("claim");
    let mut other = request(work);
    other.executor = ActorId::agent("intruder");
    assert!(matches!(
        observe_start(&plan, lineage, &ActorId::human("lead"), &other, now),
        Err(RunError::NotOwner { .. })
    ));
    let milestone = plan.find_work_by_key("TEST-M1").expect("milestone").id;
    assert!(matches!(
        observe_start(&plan, lineage, &worker(), &request(milestone), now),
        Err(RunError::NotATask(_))
    ));
    assert!(matches!(
        observe_start(&plan, lineage, &worker(), &request(WorkItemId::new()), now),
        Err(RunError::MissingWork(_))
    ));
}

#[test]
fn only_the_executor_a_human_or_a_service_may_record_a_run() {
    let (plan, work) = claimed();
    let lineage = LineageId::new();
    let now = Utc::now();
    let stranger = ActorId::agent("stranger");
    assert!(matches!(
        observe_start(&plan, lineage, &stranger, &request(work), now),
        Err(RunError::ActorNotAllowed { .. })
    ));
    for writer in [ActorId::human("lead"), ActorId::service("host")] {
        let record = observe_start(&plan, lineage, &writer, &request(work), now).expect("run");
        assert_eq!(record.recorded_by, writer);
        assert_eq!(record.executor, worker());
    }
}

#[test]
fn only_a_service_may_label_a_run_managed() {
    let (plan, work) = claimed();
    let mut managed = request(work);
    managed.observation = Observation::Managed;
    let lineage = LineageId::new();
    for writer in [worker(), ActorId::human("lead")] {
        assert!(matches!(
            observe_start(&plan, lineage, &writer, &managed, Utc::now()),
            Err(RunError::ManagedNeedsService(_))
        ));
    }
    observe_start(
        &plan,
        lineage,
        &ActorId::service("host"),
        &managed,
        Utc::now(),
    )
    .expect("service host");
}

#[test]
fn working_and_waiting_alternate_and_terminal_states_are_final() {
    let run = RunId::new();
    let states = [
        RunState::Working,
        RunState::Waiting,
        RunState::Failed,
        RunState::Interrupted,
        RunState::Completed,
    ];
    for from in states {
        for to in states {
            let outcome = check_transition(run, from, to);
            match (from.is_terminal(), from == to) {
                (true, _) => assert!(
                    matches!(outcome, Err(RunError::Terminal { .. })),
                    "{from} {to}"
                ),
                (false, true) => assert!(
                    matches!(outcome, Err(RunError::InvalidTransition { .. })),
                    "{from} {to}"
                ),
                (false, false) => assert!(outcome.is_ok(), "{from} {to}"),
            }
        }
    }
}

#[test]
fn an_agent_cannot_report_on_another_executors_run() {
    let (plan, work) = claimed();
    let (record, _) = started(&plan, work);
    authorize(&record, &worker(), "report").expect("executor");
    authorize(&record, &ActorId::human("lead"), "report").expect("operator");
    authorize(&record, &ActorId::service("host"), "report").expect("host");
    assert!(matches!(
        authorize(&record, &ActorId::agent("other"), "report"),
        Err(RunError::ActorNotAllowed { .. })
    ));
}
