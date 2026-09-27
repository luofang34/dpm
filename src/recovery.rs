use crate::{error::CliError, output};
use dpm_app::{Application, IntegrityReport};
use std::path::{Path, PathBuf};

pub(crate) fn backup_blocking(app: &Application, to: &Path, json: bool) -> Result<(), CliError> {
    present_blocking("backup written", &app.backup_blocking(to)?, json)
}

pub(crate) fn restore_blocking(from: &Path, to: &Path, json: bool) -> Result<(), CliError> {
    present_blocking(
        "restored store",
        &dpm_app::restore_store_blocking(from, to)?,
        json,
    )
}

/// Resolve the store path first: opening a workspace for verification could write to it.
pub(crate) fn verify_blocking(
    cwd: &Path,
    path: Option<PathBuf>,
    project: Option<PathBuf>,
    database: Option<PathBuf>,
    json: bool,
) -> Result<(), CliError> {
    let path = match path {
        Some(path) => cwd.join(path),
        None => dpm_app::store_path_blocking(cwd, project.as_deref(), database.as_deref())?,
    };
    present_blocking(
        "verified store",
        &dpm_app::verify_store_blocking(&path)?,
        json,
    )
}

fn present_blocking(label: &str, report: &IntegrityReport, json: bool) -> Result<(), CliError> {
    if json {
        return output::json_blocking(report);
    }
    let baseline = if report.origin_revision.is_none() {
        " (history origin not yet recorded; the next write upgrades the schema and records it)"
    } else {
        ""
    };
    output::text_blocking(&format!(
        "{label} {}\nworkspace {} ({})\nrevision {}, {} operations, schema version {}{baseline}\nintegrity ok",
        report.path.display(),
        report.workspace_name,
        report.workspace_id,
        report.revision,
        report.operation_count,
        report.schema_version,
    ))
}
