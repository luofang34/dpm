//! Backup, restore and verification entry points shared by adapters.
//!
//! These are operator file operations on device-local paths, not plan operations: they append no
//! history, change no revision and never touch device bindings or project locators.

use super::{Application, Backing};
use crate::{AppError, ProjectLocation, ProjectSource, WorkspaceRegistry};
use dpm_store::IntegrityReport;
use std::path::{Path, PathBuf};

impl Application {
    /// Write a consistent, verified copy of this workspace's store to a new file.
    pub fn backup_blocking(&self, to: &Path) -> Result<IntegrityReport, AppError> {
        match &self.backing {
            Backing::Database(store) => Ok(store.backup_blocking(to)?),
            Backing::Preview(_) => Err(AppError::InvalidRequest(
                "a preview project has no store to back up; its JSON plan file is the source"
                    .into(),
            )),
        }
    }
}

/// Restore a verified backup into a new store file; live state is never overwritten.
pub fn restore_store_blocking(from: &Path, to: &Path) -> Result<IntegrityReport, AppError> {
    Ok(dpm_store::restore_store_blocking(from, to)?)
}

/// Verify pages, layout, snapshot and history of a store without writing to it.
pub fn verify_store_blocking(path: &Path) -> Result<IntegrityReport, AppError> {
    Ok(dpm_store::verify_store_blocking(path)?)
}

/// Resolve the store file adapters would open, without opening it.
///
/// Verification must not go through a read-write open, so it needs the path rather than an
/// [`Application`].
pub fn store_path_blocking(
    start: &Path,
    project: Option<&Path>,
    database: Option<&Path>,
) -> Result<PathBuf, AppError> {
    let location = match (project, database) {
        (Some(_), Some(_)) => {
            return Err(AppError::InvalidRequest(
                "project and database are mutually exclusive".into(),
            ));
        }
        (None, Some(path)) => return Ok(start.join(path)),
        (Some(root), None) => ProjectLocation::at_blocking(start.join(root))?,
        (None, None) => ProjectLocation::discover_blocking(start)?,
    };
    match location.source {
        ProjectSource::Database(path) => Ok(path),
        ProjectSource::Registered => {
            Ok(WorkspaceRegistry::from_environment()?.resolve_blocking(location.workspace)?)
        }
        ProjectSource::Preview(path) => Err(AppError::InvalidRequest(format!(
            "project reads the preview plan {}; it has no store to verify",
            path.display()
        ))),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
