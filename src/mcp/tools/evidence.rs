//! `attach_file`: a local file attached as evidence by location, digest and size, never copied. Its
//! arguments are parsed on their own so the shared argument struct stays within the member limit.

use dpm_app::{AppError, Application, CommandRequest, FileEvidence};
use dpm_engine::Command;
use dpm_model::{ActorId, ArtifactKind};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, path::PathBuf};

/// Tool names this module owns, with the descriptions listed by `tools/list`.
pub(super) const NAMES: [(&str, &str); 1] = [(
    "attach_file",
    "Attach a local file as evidence without copying it: its location, SHA-256 digest and size are recorded so it stays checkable. Files inside .dpm are refused; prefer a commit or CI run URL when one exists",
)];

pub(super) fn handles(name: &str) -> bool {
    NAMES.iter().any(|(tool, _)| *tool == name)
}

pub(super) fn schema(properties: &mut Map<String, Value>, needed: &mut Vec<&str>) {
    properties.insert("key".into(), json!({"type":"string"}));
    properties.insert("path".into(), json!({"type":"string","minLength":1,"description":"Absolute path, or relative to the server's directory"}));
    properties.insert("kind".into(), json!({"type":"string","enum":["GitCommit","PullRequest","File","Build","TestResult","Datasheet","Quote","PurchaseOrder","Cad","Photo","Other"]}));
    properties.insert("label".into(), json!({"type":"string","minLength":1}));
    properties.insert("outcome".into(), json!({"type":"string","enum":["passed","failed","partial"],"description":"Required for TestResult and Build"}));
    properties.insert("metadata".into(), json!({"type":"object","additionalProperties":{"type":"string"},"description":"Further attributes such as command or commit; sha256 and bytes are measured"}));
    needed.extend(["key", "path", "kind", "label"]);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileArguments {
    key: Option<String>,
    path: Option<PathBuf>,
    kind: Option<ArtifactKind>,
    label: Option<String>,
    outcome: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    base_revision: Option<u64>,
}

pub(super) fn call_blocking(
    app: &mut Application,
    actor: &ActorId,
    value: Value,
    preconditions: super::preconditions::Preconditions,
) -> Result<Value, AppError> {
    let args: FileArguments = serde_json::from_value(value)?;
    let base_revision = super::required(args.base_revision, "base_revision")?;
    app.ensure_writable()?;
    let key = super::required(args.key, "key")?;
    let mut metadata = args.metadata;
    if let Some(outcome) = args.outcome {
        metadata.insert(dpm_engine::OUTCOME_KEY.into(), outcome);
    }
    let evidence = FileEvidence {
        path: super::required(args.path, "path")?,
        kind: super::required(args.kind, "kind")?,
        label: super::required(args.label, "label")?,
        metadata,
    };
    let command = app.build_command_blocking(preconditions.operation_id, |app| {
        let work = app.work_id_blocking(&key)?;
        let artifact = app.file_artifact_blocking(actor.clone(), evidence.clone())?;
        Ok::<_, AppError>(Command::AttachArtifact { work, artifact })
    })?;
    let operation = app.execute_blocking(CommandRequest {
        actor: actor.clone(),
        base_revision,
        base_lineage: preconditions.base_lineage,
        operation_id: preconditions.operation_id,
        command,
    })?;
    Ok(serde_json::to_value(dpm_app::Envelope::from(operation))?)
}
