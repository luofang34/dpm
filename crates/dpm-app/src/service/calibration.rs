//! Calibration and calibrated forecasts, which read the operation log beside the snapshot.

use super::{Application, Observed, StatusView};
use crate::AppError;
use dpm_engine::{CalibrationReport, Operation};
use dpm_model::Plan;

/// Reads of the snapshot and the whole log tried before a concurrent writer is reported.
const CONSISTENT_READS: usize = 3;

/// Largest history page the store serves.
const PAGE: u16 = 1000;

impl Application {
    /// Calibration of estimates and waits, and flow metrics, from the snapshot and its whole log.
    pub fn calibration_blocking(&self) -> Result<Observed<CalibrationReport>, AppError> {
        let (plan, history) = self.plan_with_history_blocking()?;
        let now = self.clock.now();
        Ok(Observed {
            revision: plan.revision,
            lineage_id: self.lineage_blocking()?,
            data: dpm_engine::calibration(&plan, &history, now)?,
        })
    }

    /// `status` whose forecasts apply the measured factors that have enough samples, stating each
    /// in `calibration`; the stored plan is unchanged.
    pub fn status_calibrated_blocking(
        &self,
        probabilistic: bool,
    ) -> Result<Observed<StatusView>, AppError> {
        let (plan, history) = self.plan_with_history_blocking()?;
        let now = self.clock.now();
        let report = dpm_engine::calibration(&plan, &history, now)?;
        let lineage_id = self.lineage_blocking()?;
        Ok(Observed {
            revision: plan.revision,
            lineage_id,
            data: StatusView {
                summary: dpm_engine::status_calibrated(&plan, probabilistic, now, &report)?,
                lineage_id,
            },
        })
    }

    /// The snapshot and every operation that produced it, read through the same path `history`
    /// uses. The log pages and the snapshot are separate reads, so a commit between them is
    /// detected by their revisions and the whole read repeated.
    fn plan_with_history_blocking(&self) -> Result<(Plan, Vec<Operation>), AppError> {
        let mut observed = (0, 0);
        for _ in 0..CONSISTENT_READS {
            let (revisions, history) = self.whole_history_blocking()?;
            let plan = self.plan_blocking()?;
            match revisions {
                Some(revision) if revision == plan.revision => return Ok((plan, history)),
                Some(revision) => observed = (revision, plan.revision),
                None => observed = (plan.revision, plan.revision),
            }
        }
        let (expected, actual) = observed;
        Err(AppError::Conflict { expected, actual })
    }

    /// Every operation in append order, with the revision all pages were read at, or `None` when
    /// a commit landed between pages.
    fn whole_history_blocking(&self) -> Result<(Option<u64>, Vec<Operation>), AppError> {
        let mut after = 0;
        let mut history = Vec::new();
        let mut revision = None;
        let mut consistent = true;
        loop {
            let page = self.history_blocking(after, PAGE)?;
            consistent &= revision.is_none_or(|seen| seen == page.revision);
            revision = Some(page.revision);
            if page.entries.is_empty() {
                return Ok((revision.filter(|_| consistent), history));
            }
            after = page.next_after_sequence;
            history.extend(page.entries.into_iter().map(|e| e.operation.operation));
        }
    }
}

#[cfg(test)]
#[cfg(feature = "sqlite")]
mod tests;
