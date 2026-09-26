use super::{
    ProjectError, ProjectLocation, ProjectSource, directory_blocking, exists_blocking, io_error,
    reject_legacy_blocking,
};
use crate::{AppError, Application};
use dpm_model::Plan;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Initialize a project exactly at the supplied directory, never overwriting existing state.
/// Only the locator and local ignore rules are suitable for committing to source control.
pub fn initialize_project_blocking(root: &Path, plan: &Plan) -> Result<PathBuf, AppError> {
    plan.validate().map_err(dpm_store::StoreError::from)?;
    let root = directory_blocking(root)?;
    let directory = root.join(".dpm");
    if exists_blocking(&directory)? {
        let project = ProjectLocation::at_blocking(&root)?;
        let ProjectSource::Database(database) = project.source else {
            return Err(AppError::ReadOnlyProject);
        };
        if exists_blocking(&database)? {
            return Err(ProjectError::AlreadyExists { path: directory }.into());
        }
        Application::initialize_blocking(&database, plan)?;
        return Ok(database);
    }
    reject_legacy_blocking(&root)?;
    fs::create_dir(&directory).map_err(io_error("create project directory", &directory))?;
    let database = directory.join("state.sqlite");
    Application::initialize_blocking(&database, plan)?;
    let ignore = directory.join(".gitignore");
    fs::write(&ignore, "*\n!project.toml\n!.gitignore\n")
        .map_err(io_error("write local state ignore rules", &ignore))?;
    let locator = directory.join("project.toml");
    fs::write(&locator, "version = 1\ndatabase = \"state.sqlite\"\n")
        .map_err(io_error("write project locator", &locator))?;
    Ok(database)
}
