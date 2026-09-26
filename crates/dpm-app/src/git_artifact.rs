use crate::AppError;
use chrono::Utc;
use dpm_model::{ActorId, Artifact, ArtifactId, ArtifactKind};
use std::{collections::BTreeMap, process::Command};

/// Capture immutable HEAD evidence in the current repository without changing Git.
pub fn git_head_artifact_blocking(actor: ActorId) -> Result<Artifact, AppError> {
    let sha = git_output_blocking(["rev-parse", "HEAD"])?;
    let remote = git_output_blocking(["config", "--get", "remote.origin.url"]).ok();
    let uri = match remote {
        Some(remote) if !remote.is_empty() => format!("git:{remote}@{sha}"),
        _ => format!("git:local@{sha}"),
    };
    let mut metadata = BTreeMap::from([("commit".into(), sha.clone())]);
    if let Ok(branch) = git_output_blocking(["branch", "--show-current"]) {
        metadata.insert("branch".into(), branch);
    }
    Ok(Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::GitCommit,
        uri,
        label: format!("Git commit {}", sha.chars().take(12).collect::<String>()),
        metadata,
        created_by: actor,
        created_at: Utc::now(),
    })
}

fn git_output_blocking<const N: usize>(args: [&str; N]) -> Result<String, AppError> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(AppError::GitIo)?;
    if !output.status.success() {
        return Err(AppError::Git {
            args: args.join(" "),
            message: String::from_utf8_lossy(&output.stderr).trim().into(),
        });
    }
    Ok(String::from_utf8(output.stdout)?.trim().into())
}
