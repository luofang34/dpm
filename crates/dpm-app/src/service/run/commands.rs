//! Run commands: start, report, record activity and link.
//!
//! None of these is a project mutation. Each is idempotent on its own identity, validated against
//! the plan only where the plan matters, and leaves the plan, its revision and the task's owner and
//! lifecycle exactly as they were.

use super::RunSnapshotOutcome;
use super::{
    ActivityReceipt, ReceivedActivity, RunActivityRequest, RunLinkRequest, RunReportRequest,
    RunStartRequest, RunWrite,
};
use crate::{AppError, Application, Observed};
use chrono::Utc;
use dpm_model::{LifecycleEntry, RunId, RunLink, RunStart, RunTransition};

impl Application {
    /// Record the start of a run on a task its executor owns, claimed or started.
    ///
    /// The run records the contract and plan revision it observed. Resending the same run id with
    /// the same request answers with the recorded run, even after the task moved on; the same id
    /// with a different request is refused.
    pub fn start_run_blocking(
        &mut self,
        request: RunStartRequest,
    ) -> Result<Observed<RunWrite>, AppError> {
        self.ensure_writable()?;
        let RunStartRequest {
            actor,
            work_key,
            executor,
            run_id,
            parent,
            session,
            observation,
            sources,
            observed_at,
            base_lineage,
        } = request;
        let plan = self.plan_blocking()?;
        let work = plan
            .find_work_by_key(&work_key)
            .map(|work| work.id)
            .ok_or_else(|| AppError::UnknownWork(work_key.clone()))?;
        let start = RunStart {
            id: run_id.unwrap_or_default(),
            work,
            executor: executor.unwrap_or_else(|| actor.clone()),
            parent,
            session,
            observation,
            sources,
            observed_at,
        };
        let now = Utc::now();
        let outcome = self.with_runs_mut_blocking(base_lineage, |store, lineage| {
            let written = if let Some(recorded) = store.snapshot_blocking(start.id)? {
                // A resend is answered with the original captured record, not rebuilt from a plan
                // that may have moved on. It still goes through the store's own transaction, so
                // its archive, layout and lineage checks (and the refusal of a run from before a
                // restore) apply to a retry exactly as to a first attempt.
                let written = store.start_blocking(&recorded.record, lineage)?;
                if recorded.record.request() != start || recorded.record.recorded_by != actor {
                    return Err(dpm_store::RunStoreError::DuplicateRun {
                        recorded: Box::new(recorded.record),
                    });
                }
                written
            } else {
                // Only a first attempt is judged against the plan as it stands now.
                let record = dpm_engine::observe_run_start(&plan, lineage, &actor, &start, now)?;
                store.start_blocking(&record, lineage)?
            };
            outcome(store, start.id, written.replayed, Some(written.value), None)
        })?;
        self.finish_run_write(outcome)
    }

    /// Record a lifecycle transition of a run.
    ///
    /// Working and waiting alternate and a terminal state is final. Ending a run changes nothing
    /// about the task: it stays as it was until its owner submits and a separate verifier accepts.
    pub fn report_run_blocking(
        &mut self,
        request: RunReportRequest,
    ) -> Result<Observed<RunWrite>, AppError> {
        let RunReportRequest {
            actor,
            run,
            state,
            event_id,
            detail,
            observed_at,
            base_lineage,
        } = request;
        let transition = RunTransition {
            id: event_id.unwrap_or_default(),
            run,
            to: state,
            detail,
            observed_at,
        };
        let now = Utc::now();
        let outcome = self.with_runs_mut_blocking(base_lineage, |store, lineage| {
            let written = store.transition_blocking(&transition, &actor, now, lineage)?;
            outcome(store, run, written.replayed, Some(written.value), None)
        })?;
        self.finish_run_write(outcome)
    }

    /// Record activity: tool starts and results, public progress, input requests and heartbeats.
    ///
    /// Activity is bounded telemetry. It never changes the plan revision, verifies or submits work,
    /// ends a run, or releases ownership; it only refreshes when DPM last heard from the run.
    pub fn record_run_activity_blocking(
        &mut self,
        request: RunActivityRequest,
    ) -> Result<Observed<ActivityReceipt>, AppError> {
        let RunActivityRequest {
            actor,
            entries,
            base_lineage,
        } = request;
        let now = Utc::now();
        let written = self.with_runs_mut_blocking(base_lineage, |store, lineage| {
            store.append_activity_blocking(&entries, &actor, now, lineage)
        })?;
        let found = self.revision_blocking()?;
        Ok(Observed {
            revision: found.revision,
            lineage_id: found.lineage_id,
            data: ActivityReceipt {
                entries: written
                    .into_iter()
                    .map(|written| ReceivedActivity {
                        entry: written.value,
                        duplicate: written.replayed,
                    })
                    .collect(),
            },
        })
    }

    /// Link a committed project operation to the run that performed it.
    ///
    /// The operation must already be committed, and must be the run's executor's, on the run's
    /// task, in the run's workspace and lineage, within the run's lifetime; otherwise the link is
    /// refused and nothing is recorded. Repeating a link is safe and answers with the recorded one.
    pub fn link_run_operation_blocking(
        &mut self,
        request: RunLinkRequest,
    ) -> Result<Observed<RunWrite>, AppError> {
        self.ensure_writable()?;
        let RunLinkRequest {
            actor,
            run,
            operation,
            base_lineage,
        } = request;
        let facts = self.recorded_operation_facts_blocking(operation)?;
        let now = Utc::now();
        let outcome = self.with_runs_mut_blocking(base_lineage, |store, lineage| {
            let written = store.link_blocking(run, &facts, &actor, now, lineage)?;
            outcome(store, run, written.replayed, None, Some(written.value))
        })?;
        self.finish_run_write(outcome)
    }

    /// Project the run a write touched at the query clock, against the plan as it now stands.
    fn finish_run_write(
        &self,
        outcome: RunSnapshotOutcome,
    ) -> Result<Observed<RunWrite>, AppError> {
        let RunSnapshotOutcome {
            replayed,
            event,
            link,
            snapshot,
        } = outcome;
        self.observe_blocking(|plan, now, lineage| {
            Ok(RunWrite {
                replayed,
                run: dpm_engine::project_run(snapshot, Some(plan), lineage, now),
                event,
                link,
            })
        })
    }
}

/// Read the run back inside the same borrow of the store as the write that touched it.
fn outcome(
    store: &dpm_store::RunStore,
    run: RunId,
    replayed: bool,
    event: Option<LifecycleEntry>,
    link: Option<RunLink>,
) -> Result<RunSnapshotOutcome, dpm_store::RunStoreError> {
    let snapshot = store
        .snapshot_blocking(run)?
        .ok_or(dpm_store::RunStoreError::UnknownRun(run))?;
    Ok(RunSnapshotOutcome {
        replayed,
        event,
        link,
        snapshot,
    })
}
