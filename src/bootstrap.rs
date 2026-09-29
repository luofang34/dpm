//! Commands that create a workspace or check a plan before one exists.
//!
//! Bootstrapping is local administration by the person or service operating this machine: no
//! agent tool creates or replaces a workspace, and these commands record no operation.

use crate::{
    error::{CliError, io_error},
    output,
};
use dpm_app::{Application, initialize_project_blocking};
use dpm_model::Plan;
use std::{fs, path::Path};

pub(crate) fn initialize_blocking(
    root: &Path,
    database: Option<&Path>,
    plan: Plan,
    json: bool,
) -> Result<(), CliError> {
    let database = if let Some(path) = database {
        Application::initialize_blocking(path, &plan)?;
        path.to_path_buf()
    } else {
        initialize_project_blocking(root, &plan)?
    };
    output::value_blocking(
        &serde_json::json!({"database": database, "revision": plan.revision, "workspace": plan.workspace.name}),
        Some(plan.revision),
        json,
    )
}

/// Check a plan file with the validation the `validate_plan` agent tool runs.
pub(crate) fn validate_blocking(path: &Path, json: bool) -> Result<(), CliError> {
    let text = fs::read_to_string(path).map_err(io_error("read plan", path))?;
    let report = dpm_app::validate_plan(serde_json::from_str(&text)?)?;
    output::value_blocking(&report, None, json)
}

pub(crate) fn read_plan_blocking(path: &Path) -> Result<Plan, CliError> {
    let plan = read_candidate_blocking(path)?;
    plan.validate()?;
    Ok(plan)
}

pub(crate) fn read_candidate_blocking(path: &Path) -> Result<Plan, CliError> {
    let text = fs::read_to_string(path).map_err(io_error("read plan", path))?;
    Ok(serde_json::from_str(&text)?)
}
