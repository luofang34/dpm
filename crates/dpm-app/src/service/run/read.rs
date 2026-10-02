//! Run queries: runs with their derived observation, and the two independent feeds.

use super::super::{Application, Observed};
use crate::AppError;
use dpm_model::{ActivityPage, LifecyclePage, RunId, RunView};
use serde::{Deserialize, Serialize};

/// Most runs one list returns.
const LIST_LIMIT: u16 = 1000;

/// Runs, newest first, each with its derived observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunList {
    /// The runs.
    pub runs: Vec<RunView>,
}

/// The run list's parameters.
#[derive(Debug, Clone, Default)]
pub struct RunQuery {
    /// Only runs executing this task.
    pub key: Option<String>,
    /// Most runs to return; at most 1000.
    pub limit: u16,
}

impl Application {
    /// One run with its lifecycle, derived freshness, attribution and linked operations.
    ///
    /// Freshness is computed on the query clock from the time DPM last received anything from the
    /// run, and whether the task has moved on from the plan at the observed revision.
    pub fn run_blocking(&self, id: RunId) -> Result<Observed<RunView>, AppError> {
        let snapshot = self
            .run_snapshot_blocking(id)?
            .ok_or(dpm_store::RunStoreError::UnknownRun(id))?;
        self.observe_blocking(|plan, now, lineage| {
            Ok(dpm_engine::project_run(snapshot, Some(plan), lineage, now))
        })
    }

    /// Runs, newest first, optionally only those executing one task.
    pub fn runs_blocking(&self, query: &RunQuery) -> Result<Observed<RunList>, AppError> {
        let work = query
            .key
            .as_deref()
            .map(|key| self.work_id_blocking(key))
            .transpose()?;
        let snapshots = self.run_snapshots_blocking(work, query.limit.min(LIST_LIMIT))?;
        self.observe_blocking(|plan, now, lineage| {
            Ok(RunList {
                runs: snapshots
                    .into_iter()
                    .map(|snapshot| dpm_engine::project_run(snapshot, Some(plan), lineage, now))
                    .collect(),
            })
        })
    }

    /// The lifecycle feed after a cursor: every run's durable facts in the order they were
    /// recorded. Lifecycle is never pruned, so the feed is continuous.
    pub fn run_lifecycle_blocking(
        &self,
        after_sequence: u64,
        limit: u16,
        run: Option<RunId>,
    ) -> Result<Observed<LifecyclePage>, AppError> {
        let page = self.run_lifecycle_page_blocking(after_sequence, limit, run)?;
        self.observe_feed(page)
    }

    /// The activity feed after a cursor, independent of the lifecycle feed. A cursor that retention
    /// outran gets a `gap` and resumes at the oldest record still held.
    pub fn run_activity_blocking(
        &self,
        after_sequence: u64,
        limit: u16,
        run: Option<RunId>,
    ) -> Result<Observed<ActivityPage>, AppError> {
        let page = self.run_activity_page_blocking(after_sequence, limit, run)?;
        self.observe_feed(page)
    }

    /// Stamp a feed page with the project revision and lineage without decoding the plan.
    fn observe_feed<T>(&self, data: T) -> Result<Observed<T>, AppError> {
        let found = self.revision_blocking()?;
        Ok(Observed {
            revision: found.revision,
            lineage_id: found.lineage_id,
            data,
        })
    }
}
