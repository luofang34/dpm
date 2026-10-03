//! Typed query results for native clients; adapters serialize the same values into JSON.

use super::Application;
use crate::AppError;
use chrono::{DateTime, Utc};
use dpm_engine::{
    ChangePreview, NextWorkQuery, NextWorkResult, ProgressSummary, ScheduleProjection,
    StatusSummary, WorkExplanation, WorkScope,
};
use dpm_model::{LineageId, Plan, WorkItem};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A typed view with the revision and lineage of the snapshot it was computed from.
#[derive(Debug, Clone, Serialize)]
pub struct Observed<T> {
    /// Revision the view observed.
    pub revision: u64,
    /// Lineage of that revision; absent for a read-only preview.
    pub lineage_id: Option<LineageId>,
    /// The view itself.
    pub data: T,
}

/// Where a workspace stands, cheap enough to poll: two row reads, no plan decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRevision {
    /// Committed revision.
    pub revision: u64,
    /// Lineage the revision belongs to; absent for a read-only preview, which has no store.
    pub lineage_id: Option<LineageId>,
}

/// `status`: execution counts and forecasts with the lineage they belong to.
#[derive(Debug, Clone, Serialize)]
pub struct StatusView {
    /// Counts, progress and schedule projections.
    #[serde(flatten)]
    pub summary: StatusSummary,
    /// Lineage of the observed revision.
    pub lineage_id: Option<LineageId>,
}

/// `show`: one work item with its projected progress.
#[derive(Debug, Clone, Serialize)]
pub struct WorkDetail {
    /// The work item as stored.
    #[serde(flatten)]
    pub work: WorkItem,
    /// Task, container or milestone progress projection.
    pub progress: ProgressSummary,
}

/// `next`: ranking parameters and the query-only scope applied after ranking.
#[derive(Debug, Clone, Default)]
pub struct NextRequest {
    /// Requested capabilities; empty preserves the unfiltered operator view.
    pub capabilities: BTreeSet<String>,
    /// Include seeded probabilistic criticality.
    pub probabilistic: bool,
    /// Maximum number of in-scope results.
    pub limit: usize,
    /// Project keys whose subtrees form the scope; empty does not filter.
    pub project_keys: BTreeSet<String>,
    /// WorkspaceAsset keys returned work must fit; empty does not filter.
    pub asset_keys: BTreeSet<String>,
    /// Actor about to choose work, for advice such as the claims it already holds.
    pub actor: Option<dpm_model::ActorId>,
}

impl Application {
    /// The committed revision and lineage, without loading or decoding the plan.
    pub fn revision_blocking(&self) -> Result<WorkspaceRevision, AppError> {
        match &self.backing {
            #[cfg(feature = "sqlite")]
            super::Backing::Database(store) => {
                let found = store.revision_blocking()?.ok_or(AppError::NotInitialized)?;
                Ok(WorkspaceRevision {
                    revision: found.revision,
                    lineage_id: Some(found.lineage.lineage_id),
                })
            }
            super::Backing::Preview(plan) => Ok(WorkspaceRevision {
                revision: plan.revision,
                lineage_id: None,
            }),
        }
    }
    /// Execution counts and, when `probabilistic`, the seeded Monte Carlo forecast.
    pub fn status_blocking(&self, probabilistic: bool) -> Result<Observed<StatusView>, AppError> {
        self.observe_blocking(|plan, now, lineage_id| {
            Ok(StatusView {
                summary: dpm_engine::status(plan, probabilistic, now)?,
                lineage_id,
            })
        })
    }
    /// The remaining-work schedule projection and, when `probabilistic`, its seeded uncertainty.
    pub fn schedule_blocking(
        &self,
        probabilistic: bool,
    ) -> Result<Observed<ScheduleProjection>, AppError> {
        self.observe_blocking(|plan, now, _| {
            Ok(dpm_engine::schedule_projection(plan, probabilistic, now)?)
        })
    }
    /// Globally ranked executable leaf tasks, narrowed to the requested scope.
    pub fn next_blocking(
        &self,
        request: &NextRequest,
    ) -> Result<Observed<NextWorkResult>, AppError> {
        self.observe_blocking(|plan, now, _| {
            let scope = WorkScope::resolve(
                plan,
                request.project_keys.iter().map(String::as_str),
                request.asset_keys.iter().map(String::as_str),
            )?;
            let query = NextWorkQuery {
                capabilities: request.capabilities.clone(),
                use_probabilistic_criticality: request.probabilistic,
            };
            let mut result = dpm_engine::next_in_scope(plan, &query, &scope, request.limit, now)?;
            if let Some(actor) = &request.actor {
                result.advisories = dpm_engine::advisories(plan, actor);
            }
            Ok(result)
        })
    }
    /// One work contract with its projected status and progress.
    pub fn show_blocking(&self, key: &str) -> Result<Observed<WorkDetail>, AppError> {
        self.observe_blocking(|plan, now, _| {
            let id = find_work(plan, key)?;
            let work = dpm_engine::show_work(plan, id, now)?;
            let progress = dpm_engine::progress(plan, now)?
                .work
                .remove(&id)
                .ok_or(dpm_engine::EngineError::MissingWorkItem(id))?;
            Ok(WorkDetail { work, progress })
        })
    }
    /// Readiness, gates, dependencies, schedule and ranking factors of one work item.
    pub fn explain_blocking(&self, key: &str) -> Result<Observed<WorkExplanation>, AppError> {
        self.observe_blocking(|plan, now, _| {
            Ok(dpm_engine::explain_work(plan, find_work(plan, key)?, now)?)
        })
    }
    /// The authoritative snapshot, as `export` writes it.
    pub fn export_blocking(&self) -> Result<Observed<Plan>, AppError> {
        self.observe_blocking(|plan, _, _| Ok(plan.clone()))
    }
    /// Validate a proposal against the current snapshot and describe its differences.
    pub fn propose_change_blocking(
        &self,
        proposed: &Plan,
    ) -> Result<Observed<ChangePreview>, AppError> {
        self.observe_blocking(|plan, _, _| Ok(dpm_engine::propose_change(plan, proposed)?))
    }
    /// Compute one view from a snapshot at one clock reading, so every gate, completion and
    /// schedule in it agrees.
    pub(super) fn observe_blocking<T>(
        &self,
        view: impl FnOnce(&Plan, DateTime<Utc>, Option<LineageId>) -> Result<T, AppError>,
    ) -> Result<Observed<T>, AppError> {
        let plan = self.plan_blocking()?;
        let lineage_id = self.lineage_blocking()?;
        let data = view(&plan, self.clock.now(), lineage_id)?;
        Ok(Observed {
            revision: plan.revision,
            lineage_id,
            data,
        })
    }
}

fn find_work(plan: &Plan, key: &str) -> Result<dpm_model::WorkItemId, AppError> {
    plan.find_work_by_key(key)
        .map(|work| work.id)
        .ok_or_else(|| AppError::UnknownWork(key.into()))
}

#[cfg(test)]
#[cfg(feature = "sqlite")]
mod tests;
