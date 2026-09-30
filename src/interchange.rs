use crate::{
    args::PlanCommand,
    error::{CliError, io_error},
    output,
};
use dpm_app::{Application, Query};
use std::fs;

/// Map MSPDI flags to the shared application queries used by the agent adapter too.
pub(crate) fn plan_command_blocking(
    app: &Application,
    command: PlanCommand,
    json: bool,
) -> Result<(), CliError> {
    match command {
        PlanCommand::ImportMspdi {
            file,
            project_key,
            key_prefix,
            match_existing_by,
            keep_existing_priority,
            time_zone,
            candidate,
        } => {
            let xml = fs::read_to_string(&file).map_err(io_error("read MSPDI document", &file))?;
            let response = app.query_blocking(Query::ImportMspdi {
                xml,
                project_key,
                key_prefix,
                match_existing_by,
                keep_existing_priority,
                time_zone,
            })?;
            if let Some(path) = candidate {
                let candidate = response
                    .data
                    .get("candidate")
                    .ok_or_else(|| CliError::Input("import returned no candidate".into()))?;
                let text = serde_json::to_string_pretty(candidate)?;
                fs::write(&path, text + "\n").map_err(io_error("write candidate plan", &path))?;
            }
            output::response_blocking(response, json)
        }
        PlanCommand::ExportMspdi {
            project_key,
            output: path,
        } => {
            let response = app.query_blocking(Query::ExportMspdi { project_key })?;
            let xml = response
                .data
                .get("xml")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| CliError::Input("export returned no document".into()))?;
            if let Some(path) = &path {
                fs::write(path, xml).map_err(io_error("write MSPDI document", path))?;
            }
            match (json, path) {
                (false, None) => output::raw_blocking(xml),
                (false, Some(_)) => {
                    let report = response
                        .data
                        .get("report")
                        .ok_or_else(|| CliError::Input("export returned no report".into()))?;
                    output::text_blocking(&serde_json::to_string_pretty(report)?)
                }
                (true, _) => output::response_blocking(response, true),
            }
        }
        _ => Err(CliError::Input("expected an MSPDI plan command".into())),
    }
}

/// Parse `--match-existing-by` with the same spelling the agent tool accepts.
pub(crate) fn existing_match(value: &str) -> Result<dpm_app::ExistingMatch, String> {
    serde_json::from_value(serde_json::Value::String(value.into()))
        .map_err(|_| format!("unknown rule {value:?}; expected title-path"))
}
