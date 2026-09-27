use crate::{AppError, Query};
use dpm_engine::ChangePreview;
use dpm_interchange::{ImportOptions, ImportReport, ImportResult, InterchangeError};
use dpm_model::Plan;
use serde::Serialize;
use serde_json::Value;

/// Import data shared by adapters: the report, the reviewed preview and the full candidate.
#[derive(Serialize)]
struct ImportResponse {
    report: ImportReport,
    preview: ChangePreview,
    candidate: Plan,
}

/// Answer an MSPDI query from one consistent snapshot.
///
/// An import candidate passes the same `propose_change` validation as an edited export and is
/// applied only by a human or service through `apply_change`; importing never persists anything.
pub(crate) fn query(plan: &Plan, query: Query) -> Result<Value, AppError> {
    match query {
        Query::ImportMspdi {
            xml,
            project_key,
            key_prefix,
        } => {
            let options = ImportOptions {
                project_key,
                key_prefix,
            };
            let result = dpm_interchange::import_mspdi(plan, &xml, &options)?;
            let preview = dpm_engine::propose_change(plan, &result.candidate)
                .map_err(|error| refusal(plan, &result, error))?;
            Ok(serde_json::to_value(ImportResponse {
                report: result.report,
                preview,
                candidate: result.candidate,
            })?)
        }
        Query::ExportMspdi { project_key } => Ok(serde_json::to_value(
            dpm_interchange::export_mspdi(plan, &project_key)?,
        )?),
        _ => Err(AppError::InvalidRequest(
            "expected an MSPDI import or export query".into(),
        )),
    }
}

/// A refusal at work an imported task maps to also names the source task and its attempted
/// change; the engine refusal stays the source of the error.
fn refusal(plan: &Plan, result: &ImportResult, error: dpm_engine::EngineError) -> AppError {
    let change = match &error {
        dpm_engine::EngineError::InvalidCommand { entity, .. } => {
            result.source_change(plan, entity)
        }
        _ => None,
    };
    match change {
        Some(change) => AppError::Interchange(InterchangeError::Refused {
            change,
            source: Box::new(error),
        }),
        None => AppError::Engine(error),
    }
}

#[cfg(test)]
mod tests;
