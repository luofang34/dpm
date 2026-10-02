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
        return output::json_blocking(&dpm_app::Envelope {
            lineage_id: Some(report.lineage_id),
            ..dpm_app::Envelope::new(Some(report.revision), report)
        });
    }
    let role = if report.archived {
        "archive of lineage"
    } else {
        "lineage"
    };
    output::text_blocking(&format!(
        "{label} {}\nworkspace {} ({}), {role} {}\nrevision {}, {} operations from genesis revision {}, schema version {}\nintegrity ok; replaying the history reproduces the snapshot",
        report.path.display(),
        report.workspace_name,
        report.workspace_id,
        report.lineage_id,
        report.revision,
        report.operation_count,
        report.genesis_revision,
        report.schema_version,
    ))?;
    // A project store file never holds runs, so say what happened to the run store beside it.
    output::text_blocking(&match &report.runs {
        Some(runs) => format!(
            "run store {}: {} run(s), {} lifecycle fact(s), {} activity record(s) held, schema version {}; runs are carried with the store",
            runs.path.display(),
            runs.runs,
            runs.lifecycle_events,
            runs.activity_retained,
            runs.schema_version,
        ),
        None => {
            "no run store was found beside this store: only the project history is present and carried, and this does not show that no run was ever recorded".to_string()
        }
    })
}
