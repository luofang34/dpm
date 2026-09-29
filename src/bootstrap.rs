//! Commands that create a workspace or check a plan before one exists.
//!
//! Bootstrapping is local administration by the person or service operating this machine: no
//! agent tool creates or replaces a workspace, and these commands record no operation.

use crate::{
    error::{CliError, io_error},
    output,
};
use dpm_app::{Application, Envelope, initialize_project_blocking};
use dpm_model::Plan;
use std::{fs, path::Path};

pub(crate) fn initialize_blocking(
    root: &Path,
    database: Option<&Path>,
    plan: Plan,
    json: bool,
) -> Result<(), CliError> {
    let (database, app) = if let Some(path) = database {
        let app = Application::initialize_blocking(path, &plan)?;
        (path.to_path_buf(), app)
    } else {
        let path = initialize_project_blocking(root, &plan)?;
        let app = Application::open_blocking(&path)?;
        (path, app)
    };
    // The new store's lineage is what a client caches next to revision 0 for its first write.
    let lineage_id = app.lineage_blocking()?;
    let data = serde_json::json!({"database": database, "revision": plan.revision,
        "workspace": plan.workspace.name, "lineage_id": lineage_id});
    if json {
        output::json_blocking(&Envelope {
            lineage_id,
            ..Envelope::new(Some(plan.revision), data)
        })
    } else {
        output::text_blocking(&format!("{data:#?}"))
    }
}

/// Check a plan file with the validation the `validate_plan` agent tool runs.
pub(crate) fn validate_blocking(path: &Path, json: bool) -> Result<(), CliError> {
    let report = dpm_app::validate_decoded(read_candidate_blocking(path)?)?;
    if json {
        output::success_blocking(None, &report)
    } else {
        output::json_blocking(&report)
    }
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
