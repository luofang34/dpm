use crate::{app::actor, args::EvidenceCommand, error::CliError};
use dpm_app::{Application, FileEvidence};
use dpm_engine::Command;
use dpm_model::{ActorId, ArtifactKind};
use std::{collections::BTreeMap, fs};

/// Build the evidence the agent adapter builds from the same inputs: an Artifact read from a file,
/// the checkout's Git HEAD, or a local file fingerprinted in place.
pub(crate) fn mutation_blocking(
    app: &Application,
    command: EvidenceCommand,
) -> Result<(ActorId, Command), CliError> {
    match command {
        EvidenceCommand::Artifact {
            key,
            file,
            actor: who,
        } => {
            let text = fs::read_to_string(&file)
                .map_err(crate::error::io_error("read artifact", &file))?;
            let artifact = serde_json::from_str(&text)?;
            let principal = actor(&who)?;
            let work = app.work_id_blocking(&key)?;
            Ok((principal, Command::AttachArtifact { work, artifact }))
        }
        EvidenceCommand::AttachGitHead {
            key,
            actor: who,
            asset,
        } => {
            let principal = actor(&who)?;
            let work = app.work_id_blocking(&key)?;
            let artifact =
                app.git_head_artifact_blocking(principal.clone(), work, asset.as_deref())?;
            Ok((principal, Command::AttachArtifact { work, artifact }))
        }
        EvidenceCommand::AttachFile {
            key,
            path,
            kind,
            label,
            outcome,
            metadata,
            actor: who,
        } => {
            let principal = actor(&who)?;
            let mut attributes = BTreeMap::new();
            for pair in metadata {
                let (name, value) = pair
                    .split_once('=')
                    .ok_or_else(|| CliError::Input(format!("--meta {pair:?} is not KEY=VALUE")))?;
                attributes.insert(name.to_string(), value.to_string());
            }
            if let Some(outcome) = outcome {
                attributes.insert(dpm_engine::OUTCOME_KEY.into(), outcome);
            }
            let kind: ArtifactKind = serde_json::from_value(serde_json::Value::String(kind))?;
            let work = app.work_id_blocking(&key)?;
            let evidence = FileEvidence {
                path,
                kind,
                label,
                metadata: attributes,
            };
            let artifact = app.file_artifact_blocking(principal.clone(), evidence)?;
            Ok((principal, Command::AttachArtifact { work, artifact }))
        }
    }
}
