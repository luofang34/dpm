//! Turning a start request into the record of what the run observed.

use super::RunError;
use chrono::{DateTime, Utc};
use dpm_model::{
    ActorId, ActorKind, ContractObservation, LineageId, Observation, Plan, RunRecord, RunStart,
    WorkStatus,
};

/// Validate a start against the plan and capture the contract it observes.
///
/// The task must be an executable task its executor already holds, claimed or started. `writer` is
/// the principal recording the run: the executor itself, or a human or service acting for it.
/// `lineage` is the history `plan` was read from; the record keeps it with the plan revision so the
/// observation stays attributable after a restore. `now` is DPM's own clock, the receipt time.
pub fn observe_start(
    plan: &Plan,
    lineage: LineageId,
    writer: &ActorId,
    request: &RunStart,
    now: DateTime<Utc>,
) -> Result<RunRecord, RunError> {
    request.validate()?;
    super::transition::authorize_writer(writer, &request.executor, request.id, "start")?;
    if request.observation == Observation::Managed && writer.kind != ActorKind::Service {
        return Err(RunError::ManagedNeedsService(request.id));
    }
    let work = plan
        .work_items
        .get(&request.work)
        .ok_or(RunError::MissingWork(request.work))?;
    if !work.is_executable() {
        return Err(RunError::NotATask(work.id));
    }
    if !matches!(
        work.execution.status,
        WorkStatus::Claimed | WorkStatus::InProgress
    ) {
        return Err(RunError::NotExecuting {
            work: work.id,
            status: work.execution.status,
        });
    }
    if work.execution.owner.as_ref() != Some(&request.executor) {
        return Err(RunError::NotOwner {
            work: work.id,
            executor: request.executor.clone(),
            owner: work.execution.owner.clone(),
        });
    }
    let exact_sources = request
        .sources
        .iter()
        .map(|source| super::source::resolve(plan, work, source))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RunRecord {
        id: request.id,
        work: work.id,
        executor: request.executor.clone(),
        recorded_by: writer.clone(),
        parent: request.parent,
        session: request.session.clone(),
        observation: request.observation,
        sources: request.sources.clone(),
        exact_sources,
        contract: ContractObservation {
            workspace_id: plan.workspace.id,
            lineage_id: lineage,
            revision: plan.revision,
            work_key: work.key.clone(),
            prior_submissions: u32::try_from(work.execution.attempts.len()).unwrap_or(u32::MAX),
            contract: work.contract.clone(),
        },
        observed_started_at: request.observed_at,
        started_at: now,
    })
}
