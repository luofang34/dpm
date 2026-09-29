use crate::{AppError, Application};
use chrono::Utc;
use dpm_model::{ActorId, Artifact, ArtifactId, ArtifactKind};
use std::{collections::BTreeMap, path::Path, process::Command};

impl Application {
    /// Capture evidence from the selected project, or the current directory for an explicit DB.
    pub fn git_head_artifact_blocking(
        &self,
        actor: ActorId,
        work: dpm_model::WorkItemId,
        asset: Option<&str>,
    ) -> Result<Artifact, AppError> {
        self.ensure_writable()?;
        let plan = self.plan_blocking()?;
        let selected = if let Some(key) = asset {
            let id = plan
                .assets
                .values()
                .find(|r| r.key.0 == key)
                .ok_or_else(|| AppError::InvalidRequest(format!("unknown asset {key}")))?
                .id;
            if self.project_asset.is_some_and(|bound| bound != id) {
                return Err(AppError::InvalidRequest(
                    "asset differs from this checkout's locator binding".into(),
                ));
            }
            id
        } else {
            self.project_asset.ok_or_else(|| {
                AppError::InvalidRequest(
                    "Git evidence needs a locator asset or explicit --asset key".into(),
                )
            })?
        };
        let item = plan
            .work_items
            .get(&work)
            .ok_or(dpm_engine::EngineError::MissingWorkItem(work))?;
        if !item.contract.assets.iter().any(|r| r.asset == selected)
            || !plan.assets.get(&selected).is_some_and(|asset| {
                matches!(asset.kind, dpm_model::AssetKind::GitRepository { .. })
            })
        {
            return Err(AppError::InvalidRequest(
                "Git asset must belong to the task's requirements".into(),
            ));
        }
        let mut artifact = git_head_at_blocking(
            actor,
            self.project_root.as_deref().unwrap_or(Path::new(".")),
        )?;
        artifact
            .metadata
            .insert("asset_id".into(), selected.to_string());
        let commit = artifact
            .metadata
            .get("commit")
            .cloned()
            .ok_or_else(|| AppError::InvalidRequest("Git evidence names no commit".into()))?;
        artifact.uri = format!("git:asset:{selected}@{commit}");
        Ok(artifact)
    }
}

fn git_head_at_blocking(actor: ActorId, root: &Path) -> Result<Artifact, AppError> {
    let sha = git_output_blocking(root, ["rev-parse", "HEAD"])?;
    let remote = git_output_blocking(root, ["config", "--get", "remote.origin.url"]).ok();
    let uri = match remote {
        Some(remote) if !remote.is_empty() => format!("git:{remote}@{sha}"),
        _ => format!("git:local@{sha}"),
    };
    let mut metadata = BTreeMap::from([("commit".into(), sha.clone())]);
    if let Ok(branch) = git_output_blocking(root, ["branch", "--show-current"]) {
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

fn git_output_blocking<const N: usize>(root: &Path, args: [&str; N]) -> Result<String, AppError> {
    let output = Command::new("git")
        .current_dir(root)
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

#[cfg(test)]
mod tests;
