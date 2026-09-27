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

/// A refusal at work an imported task maps to, or at a dependency a source link maps to (or that
/// the candidate removes because the document omits it), also names the source task or link and
/// the attempted change; the engine refusal stays the source of the error.
fn refusal(plan: &Plan, result: &ImportResult, error: dpm_engine::EngineError) -> AppError {
    let dpm_engine::EngineError::InvalidCommand { entity, .. } = &error else {
        return AppError::Engine(error);
    };
    if let Some(change) = result.source_change(plan, entity) {
        return AppError::Interchange(InterchangeError::Refused {
            change,
            source: Box::new(error),
        });
    }
    match result.source_link_change(plan, entity) {
        Some(change) => AppError::Interchange(InterchangeError::RefusedLink {
            change: Box::new(change),
            source: Box::new(error),
        }),
        None => AppError::Engine(error),
    }
}

#[cfg(test)]
mod tests;
