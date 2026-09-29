use crate::{AppError, Application};
use dpm_model::{Plan, WorkspaceId};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

mod error;
#[cfg(feature = "sqlite")]
mod initialization;
pub use error::ProjectError;
use error::io_error;
#[cfg(feature = "sqlite")]
pub use initialization::initialize_project_blocking;

/// An explicitly configured source for a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectSource {
    /// Resolve an explicit workspace identity through the device registry.
    Registered,
    /// Authoritative local SQLite state with a semantic operation log.
    Database(PathBuf),
    /// An author-written JSON plan, accessible only as an in-memory read-only preview.
    Preview(PathBuf),
}

/// A project root and the source named by its versioned locator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectLocation {
    /// Expected workspace identity, checked before any operation.
    pub workspace: WorkspaceId,
    /// WorkspaceAsset represented by this checkout, if any.
    pub asset: Option<String>,
    /// Directory containing `.dpm/project.toml`.
    pub root: PathBuf,
    /// Resolved source path, relative to the locator directory when configured relatively.
    pub source: ProjectSource,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Locator {
    version: u32,
    workspace: WorkspaceId,
    asset: Option<String>,
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
        if locator.version != 3 {
            return Err(ProjectError::Invalid {
                path,
                message: format!(
                    "unsupported locator version {}; supported version is 3",
                    locator.version
                ),
            });
        }
        let source = match (locator.database, locator.preview) {
            (Some(path), None) if portable_path(&path) => {
                ProjectSource::Database(root.join(".dpm").join(path))
            }
            (None, Some(path)) if portable_path(&path) => {
                ProjectSource::Preview(root.join(".dpm").join(path))
            }
            (None, None) => ProjectSource::Registered,
            _ => {
                return Err(ProjectError::Invalid {
                    path,
                    message: "use a relative database or preview path, or omit both for a registered workspace".into(),
                });
            }
        };
        Ok(Self {
            root,
            source,
            workspace: locator.workspace,
            asset: locator.asset,
        })
    }

    /// Open through the application boundary without creating state on a read request.
    pub fn open_blocking(&self) -> Result<Application, AppError> {
        #[cfg(feature = "registry")]
        if matches!(self.source, ProjectSource::Registered) {
            return self
                .open_with_registry_blocking(&crate::WorkspaceRegistry::from_environment()?);
        }
        self.open_source_blocking()
    }

    /// The store file this locator selects, resolving a registered workspace through the device
    /// bindings; `None` for a preview plan file. Nothing is opened or decoded.
    pub fn store_path_blocking(&self) -> Result<Option<PathBuf>, AppError> {
        match &self.source {
            ProjectSource::Database(path) => Ok(Some(path.clone())),
            #[cfg(feature = "registry")]
            ProjectSource::Registered => Ok(Some(
                crate::WorkspaceRegistry::from_environment()?.resolve_blocking(self.workspace)?,
            )),
            #[cfg(not(feature = "registry"))]
            ProjectSource::Registered => Err(unbound(self.workspace)),
            ProjectSource::Preview(_) => Ok(None),
        }
    }

    /// Open using explicitly supplied device bindings without changing process environment.
    #[cfg(feature = "registry")]
    pub fn open_with_registry_blocking(
        &self,
        registry: &crate::WorkspaceRegistry,
    ) -> Result<Application, AppError> {
        if matches!(self.source, ProjectSource::Registered) {
            let path = registry.resolve_blocking(self.workspace)?;
            let app = crate::registry::open_bound_blocking(self.workspace, &path)?;
            return self.attach_blocking(app);
        }
        self.open_source_blocking()
    }

    /// Open a source named by the locator itself; a registered one needs the registry.
    fn open_source_blocking(&self) -> Result<Application, AppError> {
        let app = match &self.source {
            ProjectSource::Registered => Err(unbound(self.workspace)),
            #[cfg(feature = "sqlite")]
            ProjectSource::Database(path) => Application::open_blocking(path),
            #[cfg(not(feature = "sqlite"))]
            ProjectSource::Database(_) => Err(AppError::Unsupported { feature: "sqlite" }),
            ProjectSource::Preview(path) => {
                let text = fs::read_to_string(path).map_err(io_error("read preview plan", path))?;
                let plan: Plan = serde_json::from_str(&text)?;
                Application::preview(plan)
            }
        }?;
        self.attach_blocking(app)
    }

    fn attach_blocking(&self, mut app: Application) -> Result<Application, AppError> {
        let plan = app.plan_blocking()?;
        if plan.workspace.id != self.workspace {
            return Err(ProjectError::Invalid {
                path: self.root.clone(),
                message: format!(
                    "locator expects workspace {}, store contains {}",
                    self.workspace, plan.workspace.id
                ),
            }
            .into());
        }
        if let Some(key) = &self.asset {
            app.project_asset = Some(
                plan.assets
                    .values()
                    .find(|r| r.key.0 == *key)
                    .ok_or_else(|| ProjectError::Invalid {
                        path: self.root.clone(),
                        message: format!("unknown asset {key}"),
                    })?
                    .id,
            );
        }
        app.project_root = Some(self.root.clone());
        Ok(app)
    }
}

/// The refusal for a registered workspace no binding is available for.
#[cfg_attr(not(feature = "registry"), allow(unused_variables))]
fn unbound(workspace: WorkspaceId) -> AppError {
    #[cfg(feature = "registry")]
    return crate::RegistryError::NotBound { workspace }.into();
    #[cfg(not(feature = "registry"))]
    return AppError::Unsupported {
        feature: "registry",
    };
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
        #[cfg(feature = "sqlite")]
        (None, Some(path)) => Application::open_blocking(start.join(path)),
        #[cfg(not(feature = "sqlite"))]
        (None, Some(_)) => Err(AppError::Unsupported { feature: "sqlite" }),
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

fn portable_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && !path.to_string_lossy().contains('\\')
        && !path.to_string_lossy().contains(':')
}
