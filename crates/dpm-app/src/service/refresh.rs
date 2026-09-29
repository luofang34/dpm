//! Reopening the selected source for a console that follows changes made elsewhere.
//!
//! A project locator may be repointed at another store, such as a restored copy, so the probe and
//! the reload both resolve the locator again rather than trusting the connection opened at startup.
//! Without a locator (an explicit database path) the open connection is the source.

use super::{Application, Observed, WorkspaceRevision};
use crate::{AppError, ProjectError, ProjectLocation, ProjectSource};
use dpm_model::Plan;
use serde::Deserialize;
use std::{fs, path::Path};

impl Application {
    /// Revision and lineage of the source a refresh would load, without decoding its plan.
    ///
    /// Cheap enough to poll: it rereads the locator and two store rows. A change in lineage, or a
    /// revision lower than the one displayed, means the source now holds another history.
    pub fn refreshed_revision_blocking(&self) -> Result<WorkspaceRevision, AppError> {
        let Some(root) = &self.project_root else {
            return self.revision_blocking();
        };
        let location = ProjectLocation::at_blocking(root)?;
        if let ProjectSource::Preview(path) = &location.source {
            return Ok(WorkspaceRevision {
                revision: preview_revision_blocking(path)?,
                lineage_id: None,
            });
        }
        #[cfg(feature = "sqlite")]
        if let Some(path) = location.store_path_blocking()? {
            let found =
                dpm_store::store_revision_blocking(&path)?.ok_or(AppError::NotInitialized)?;
            return Ok(WorkspaceRevision {
                revision: found.revision,
                lineage_id: Some(found.lineage.lineage_id),
            });
        }
        location.open_blocking()?.revision_blocking()
    }

    /// Reopen the selected source for a UI reload without changing workspace identity or mode,
    /// returning the plan with the lineage of the same source it was loaded from.
    pub fn refreshed_snapshot_blocking(&self) -> Result<Observed<Plan>, AppError> {
        let current = self.plan_blocking()?;
        let Some(root) = &self.project_root else {
            return Ok(Observed {
                revision: current.revision,
                lineage_id: self.lineage_blocking()?,
                data: current,
            });
        };
        let source = ProjectLocation::at_blocking(root)?.open_blocking()?;
        let next = source.plan_blocking()?;
        if source.is_read_only() != self.is_read_only() || next.workspace.id != current.workspace.id
        {
            return Err(AppError::InvalidRequest(
                "source identity or mode changed; reopen explicitly".into(),
            ));
        }
        Ok(Observed {
            revision: next.revision,
            lineage_id: source.lineage_blocking()?,
            data: next,
        })
    }
}

/// The revision a preview plan file records, read without decoding or validating the plan; a
/// malformed plan is reported by the reload that follows a change.
fn preview_revision_blocking(path: &Path) -> Result<u64, AppError> {
    #[derive(Deserialize)]
    struct Head {
        revision: u64,
    }
    let text = fs::read_to_string(path).map_err(|source| ProjectError::Io {
        action: "read preview plan",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(serde_json::from_str::<Head>(&text)?.revision)
}

#[cfg(all(test, feature = "sqlite"))]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
