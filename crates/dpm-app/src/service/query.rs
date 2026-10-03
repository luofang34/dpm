//! The JSON query boundary adapters call; every view is the serialized typed result.

use super::{API_VERSION, Application, NextRequest, Observed, Query, QueryResponse};
use crate::AppError;
use dpm_store::HistoryPage;
use serde::Serialize;

impl Application {
    /// Compute one query from a consistent snapshot.
    pub fn query_blocking(&self, query: Query) -> Result<QueryResponse, AppError> {
        match query {
            Query::Revision => {
                let found = self.revision_blocking()?;
                respond(Observed {
                    revision: found.revision,
                    lineage_id: found.lineage_id,
                    data: found,
                })
            }
            Query::History {
                after_sequence,
                limit,
            } => {
                let page = self.history_blocking(after_sequence, limit)?;
                respond(Observed {
                    revision: page.revision,
                    lineage_id: page.lineage_id,
                    data: page,
                })
            }
            Query::Runs { key, limit } => {
                respond(self.runs_blocking(&crate::RunQuery { key, limit })?)
            }
            Query::Run { id } => respond(self.run_blocking(id)?),
            Query::RunLifecycle {
                after_sequence,
                limit,
                run,
            } => respond(self.run_lifecycle_blocking(after_sequence, limit, run)?),
            Query::RunActivity {
                after_sequence,
                limit,
                run,
            } => respond(self.run_activity_blocking(after_sequence, limit, run)?),
            Query::ProposeChange { plan } => respond(self.propose_change_blocking(&plan)?),
            Query::Export => respond(self.export_blocking()?),
            Query::PlanSchema => respond(self.observe_blocking(|_, _, _| crate::plan_schema())?),
            Query::PlanTemplate => {
                respond(self.observe_blocking(|plan, _, _| crate::authoring::plan_template(plan))?)
            }
            interchange @ (Query::ImportMspdi { .. } | Query::ExportMspdi { .. }) => respond(
                self.observe_blocking(|plan, _, _| crate::interchange::query(plan, interchange))?,
            ),
            Query::Status {
                probabilistic,
                calibrated: false,
            } => respond(self.status_blocking(probabilistic)?),
            Query::Status {
                probabilistic,
                calibrated: true,
            } => respond(self.status_calibrated_blocking(probabilistic)?),
            Query::Schedule { probabilistic } => respond(self.schedule_blocking(probabilistic)?),
            Query::Calibration => respond(self.calibration_blocking()?),
            Query::Next {
                capabilities,
                probabilistic,
                limit,
                project_keys,
                asset_keys,
                actor,
            } => respond(self.next_blocking(&NextRequest {
                capabilities,
                probabilistic,
                limit,
                project_keys,
                asset_keys,
                actor,
            })?),
            Query::Show { key } => respond(self.show_blocking(&key)?),
            Query::Explain { key } => respond(self.explain_blocking(&key)?),
        }
    }
    /// Read one history page with its revision and lineage in the store's own transaction; a
    /// preview has no history.
    #[cfg_attr(not(feature = "sqlite"), allow(unused_variables))]
    pub fn history_blocking(
        &self,
        after_sequence: u64,
        limit: u16,
    ) -> Result<HistoryPage, AppError> {
        Ok(match &self.backing {
            #[cfg(feature = "sqlite")]
            super::Backing::Database(store) => store.history_blocking(after_sequence, limit)?,
            super::Backing::Preview(plan) => HistoryPage {
                revision: plan.revision,
                lineage_id: None,
                entries: Vec::new(),
                next_after_sequence: after_sequence,
            },
        })
    }
}

fn respond<T: Serialize>(observed: Observed<T>) -> Result<QueryResponse, AppError> {
    Ok(QueryResponse {
        api_version: API_VERSION,
        revision: observed.revision,
        lineage_id: observed.lineage_id,
        data: serde_json::to_value(observed.data)?,
    })
}
