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
            candidate,
        } => {
            let xml = fs::read_to_string(&file).map_err(io_error("read MSPDI document", &file))?;
            let response = app.query_blocking(Query::ImportMspdi {
                xml,
                project_key,
                key_prefix,
                match_existing_by,
            })?;
            if let Some(path) = candidate {
                let text = serde_json::to_string_pretty(&response.data["candidate"])?;
                fs::write(&path, text + "\n").map_err(io_error("write candidate plan", &path))?;
            }
            present_blocking(&response.data, json)
        }
        PlanCommand::ExportMspdi {
            project_key,
            output: path,
        } => {
            let response = app.query_blocking(Query::ExportMspdi { project_key })?;
            let xml = response.data["xml"]
                .as_str()
                .ok_or_else(|| CliError::Input("export returned no document".into()))?;
            if let Some(path) = &path {
                fs::write(path, xml).map_err(io_error("write MSPDI document", path))?;
            }
            match (json, path) {
                (true, _) => output::json_blocking(&response.data),
                (false, None) => output::raw_blocking(xml),
                (false, Some(_)) => present_blocking(&response.data["report"], false),
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

fn present_blocking(data: &serde_json::Value, json: bool) -> Result<(), CliError> {
    if json {
        output::json_blocking(data)
    } else {
        output::text_blocking(&serde_json::to_string_pretty(data)?)
    }
}
