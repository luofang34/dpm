use crate::{AppError, Application};
use dpm_model::Plan;
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

mod error;
mod initialization;
pub use error::ProjectError;
use error::io_error;
pub use initialization::initialize_project_blocking;

/// An explicitly configured source for a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectSource {
    /// Authoritative local SQLite state with a semantic operation log.
    Database(PathBuf),
    /// An author-written JSON plan, accessible only as an in-memory read-only preview.
    Preview(PathBuf),
}

/// A project root and the source named by its versioned locator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectLocation {
    /// Directory containing `.dpm/project.toml`.
    pub root: PathBuf,
    /// Resolved source path, relative to the locator directory when configured relatively.
    pub source: ProjectSource,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Locator {
    version: u32,
    database: Option<PathBuf>,
    preview: Option<PathBuf>,
}

impl ProjectLocation {
    /// Find the nearest locator at or above a directory, stopping at the first Git boundary.
    pub fn discover_blocking(start: impl AsRef<Path>) -> Result<Self, ProjectError> {
        let start = directory_blocking(start.as_ref())?;
        for root in start.ancestors() {
            if exists_blocking(&root.join(".dpm"))? {
                return Self::at_blocking(root);
            }
            reject_legacy_blocking(root)?;
            if exists_blocking(&root.join(".git"))? {
                break;
            }
        }
        Err(ProjectError::NotFound { start })
    }

    /// Open exactly this project directory; never fall back to a parent configuration.
    pub fn at_blocking(root: impl AsRef<Path>) -> Result<Self, ProjectError> {
        let root = directory_blocking(root.as_ref())?;
        let path = root.join(".dpm/project.toml");
        let text = fs::read_to_string(&path).map_err(io_error("read project locator", &path))?;
        let locator: Locator = toml::from_str(&text).map_err(|source| ProjectError::Parse {
            path: path.clone(),
            source,
        })?;
        if locator.version != 1 {
            return Err(ProjectError::Invalid {
                path,
                message: format!(
                    "unsupported locator version {}; supported version is 1",
                    locator.version
                ),
            });
        }
        let source = match (locator.database, locator.preview) {
            (Some(path), None) if !path.as_os_str().is_empty() => {
                ProjectSource::Database(root.join(".dpm").join(path))
            }
            (None, Some(path)) if !path.as_os_str().is_empty() => {
                ProjectSource::Preview(root.join(".dpm").join(path))
            }
            _ => {
                return Err(ProjectError::Invalid {
                    path,
                    message: "specify exactly one nonempty database or preview path".into(),
                });
            }
        };
        Ok(Self { root, source })
    }

    /// Open through the application boundary without creating state on a read request.
    pub fn open_blocking(&self) -> Result<Application, AppError> {
        let mut app = match &self.source {
            ProjectSource::Database(path) => Application::open_blocking(path),
            ProjectSource::Preview(path) => {
                let text = fs::read_to_string(path).map_err(io_error("read preview plan", path))?;
                let plan: Plan = serde_json::from_str(&text)?;
                Application::preview(plan)
            }
        }?;
        app.project_root = Some(self.root.clone());
        Ok(app)
    }
}

/// Resolve explicit adapter arguments or discover from the supplied current directory.
/// Supplying both overrides is an error; an explicit database never consults project locators.
pub fn open_workspace_blocking(
    start: &Path,
    project: Option<&Path>,
    database: Option<&Path>,
) -> Result<Application, AppError> {
    match (project, database) {
        (Some(_), Some(_)) => Err(AppError::InvalidRequest(
            "project and database are mutually exclusive".into(),
        )),
        (None, Some(path)) => Application::open_blocking(start.join(path)),
        (Some(root), None) => ProjectLocation::at_blocking(start.join(root))?.open_blocking(),
        (None, None) => ProjectLocation::discover_blocking(start)?.open_blocking(),
    }
}

fn directory_blocking(path: &Path) -> Result<PathBuf, ProjectError> {
    let root = fs::canonicalize(path).map_err(io_error("resolve project directory", path))?;
    if !root.is_dir() {
        return Err(ProjectError::Invalid {
            path: root,
            message: "project location must be a directory".into(),
        });
    }
    Ok(root)
}

fn exists_blocking(path: &Path) -> Result<bool, ProjectError> {
    path.try_exists()
        .map_err(io_error("inspect project path", path))
}

fn reject_legacy_blocking(root: &Path) -> Result<(), ProjectError> {
    let path = root.join(".dagplan/dagplan.sqlite");
    if exists_blocking(&path)? {
        return Err(ProjectError::Legacy { path });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
