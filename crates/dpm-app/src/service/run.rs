//! Agent runs through the shared application boundary.
//!
//! Runs are observations kept in a sidecar run store, not project operations. Nothing here goes
//! through `commit_blocking`: a run write never takes the project writer lock, never changes the
//! plan revision, and never notifies commit watchers. Reads answer from the same store, with
//! freshness judged on the application's query clock and attribution compared with the project
//! store's current lineage.

use super::{Application, Observed};
use crate::AppError;
use serde::Serialize;

mod request;
pub use request::{
    RunActivityRequest, RunCommand, RunLinkRequest, RunReportRequest, RunStartRequest,
};
mod read;
pub use read::{RunList, RunQuery};
mod write;
pub use write::{ActivityReceipt, ReceivedActivity, RunWrite};

#[cfg(feature = "sqlite")]
mod commands;
#[cfg(feature = "sqlite")]
mod slot;
#[cfg(feature = "sqlite")]
pub(super) use slot::RunSlot;
#[cfg(not(feature = "sqlite"))]
mod stub;

/// What a run write leaves behind, read back inside the same borrow of the run store.
#[cfg(feature = "sqlite")]
struct RunSnapshotOutcome {
    replayed: bool,
    event: Option<dpm_model::LifecycleEntry>,
    link: Option<dpm_model::RunLink>,
    snapshot: dpm_model::RunSnapshot,
}

/// Without SQLite there is no run store; every write is a read-only refusal and every read is empty.
#[cfg(not(feature = "sqlite"))]
#[derive(Default)]
pub(super) struct RunSlot(());

impl Application {
    /// Run one run command and return its typed result, as the JSON adapters serialize it.
    pub fn execute_run_blocking(
        &mut self,
        command: RunCommand,
    ) -> Result<super::QueryResponse, AppError> {
        match command {
            RunCommand::Start(request) => respond(self.start_run_blocking(request)?),
            RunCommand::Report(request) => respond(self.report_run_blocking(request)?),
            RunCommand::Activity(request) => respond(self.record_run_activity_blocking(request)?),
            RunCommand::Link(request) => respond(self.link_run_operation_blocking(request)?),
        }
    }
}

fn respond<T: Serialize>(observed: Observed<T>) -> Result<super::QueryResponse, AppError> {
    Ok(super::QueryResponse {
        api_version: super::API_VERSION,
        revision: observed.revision,
        lineage_id: observed.lineage_id,
        data: serde_json::to_value(observed.data)?,
    })
}

#[cfg(test)]
#[cfg(feature = "sqlite")]
mod tests;
