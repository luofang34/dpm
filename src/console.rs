//! The operator console's source: the application, resolving the selected project again on every
//! probe and reload so a repointed locator is noticed.

use crate::error::CliError;
use dpm_app::{AppError, Application, QueryClock};
use dpm_tui::{Snapshot, SnapshotSource, SourceRevision};

struct ApplicationSource<'a>(&'a Application);

impl SnapshotSource for ApplicationSource<'_> {
    type Error = AppError;

    fn revision_blocking(&mut self) -> Result<SourceRevision, AppError> {
        let found = self.0.refreshed_revision_blocking()?;
        Ok(SourceRevision {
            revision: found.revision,
            lineage_id: found.lineage_id,
        })
    }

    fn snapshot_blocking(&mut self) -> Result<Snapshot, AppError> {
        let observed = self.0.refreshed_snapshot_blocking()?;
        Ok(Snapshot {
            plan: observed.data,
            lineage_id: observed.lineage_id,
        })
    }
}

/// Open the console on the application's source until the operator quits; at a caller's pinned
/// clock, on the workspace as it stood then.
pub(crate) fn run_blocking(app: &Application) -> Result<(), CliError> {
    if let QueryClock::Pinned(at) = app.query_clock() {
        dpm_tui::run_as_of_blocking(&app.export_blocking()?.data, at)?;
        return Ok(());
    }
    let mut source = ApplicationSource(app);
    let initial = source.snapshot_blocking()?;
    dpm_tui::run_following_blocking(initial, app.is_read_only(), &mut source)?;
    Ok(())
}
