//! A build without SQLite has no run store: reads are empty and every write is refused as the
//! read-only preview refuses project mutations.

use super::{
    ActivityReceipt, RunActivityRequest, RunLinkRequest, RunReportRequest, RunStartRequest,
    RunWrite,
};
use crate::{AppError, Application, Observed};
use dpm_model::{ActivityPage, LifecyclePage, RunId, RunSnapshot, WorkItemId};

impl Application {
    /// Without a run store there is no retention to configure.
    pub fn set_run_retention(&mut self, _max_activity: u64) {}

    pub(in crate::service) fn run_heads_blocking(
        &self,
    ) -> Result<dpm_model::RunFeedHeads, AppError> {
        Ok(dpm_model::RunFeedHeads::default())
    }

    pub(super) fn run_snapshot_blocking(&self, _: RunId) -> Result<Option<RunSnapshot>, AppError> {
        Ok(None)
    }

    pub(super) fn run_snapshots_blocking(
        &self,
        _: Option<WorkItemId>,
        _: u16,
    ) -> Result<Vec<RunSnapshot>, AppError> {
        Ok(Vec::new())
    }

    pub(in crate::service) fn run_lifecycle_page_blocking(
        &self,
        after: u64,
        _: u16,
        _: Option<RunId>,
    ) -> Result<LifecyclePage, AppError> {
        Ok(LifecyclePage {
            entries: Vec::new(),
            next_after_sequence: after,
            head_sequence: 0,
        })
    }

    pub(in crate::service) fn run_activity_page_blocking(
        &self,
        after: u64,
        _: u16,
        _: Option<RunId>,
    ) -> Result<ActivityPage, AppError> {
        Ok(ActivityPage {
            entries: Vec::new(),
            next_after_sequence: after,
            head_sequence: 0,
            gap: None,
        })
    }

    /// Refused: this build has no run store.
    pub fn start_run_blocking(
        &mut self,
        _: RunStartRequest,
    ) -> Result<Observed<RunWrite>, AppError> {
        Err(AppError::ReadOnlyProject)
    }

    /// Refused: this build has no run store.
    pub fn report_run_blocking(
        &mut self,
        _: RunReportRequest,
    ) -> Result<Observed<RunWrite>, AppError> {
        Err(AppError::ReadOnlyProject)
    }

    /// Refused: this build has no run store.
    pub fn record_run_activity_blocking(
        &mut self,
        _: RunActivityRequest,
    ) -> Result<Observed<ActivityReceipt>, AppError> {
        Err(AppError::ReadOnlyProject)
    }

    /// Refused: this build has no run store.
    pub fn link_run_operation_blocking(
        &mut self,
        _: RunLinkRequest,
    ) -> Result<Observed<RunWrite>, AppError> {
        Err(AppError::ReadOnlyProject)
    }
}
