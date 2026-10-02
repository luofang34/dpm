//! Resolving a run's source references to exact commits the plan supports.

use super::RunError;
use dpm_model::{
    Artifact, ArtifactKind, AssetId, AssetKind, ExactSource, Plan, RunSource, WorkItem,
    is_exact_commit,
};

/// Resolve one source reference to the exact commit it names, or refuse it.
///
/// A named commit must belong to a Git repository asset the task requires. Evidence must be a
/// `GitCommit` artifact attached to this task whose `commit` and `asset_id` metadata and
/// `git:asset:ASSET@COMMIT` URI agree on one exact commit of such an asset: a kind tag alone, or a
/// mutable locator such as a branch, proves nothing about what the run executes against.
pub(super) fn resolve(
    plan: &Plan,
    work: &WorkItem,
    source: &RunSource,
) -> Result<ExactSource, RunError> {
    match source {
        RunSource::GitCommit { asset, commit } => {
            repository(plan, work, *asset)?;
            Ok(ExactSource {
                asset: *asset,
                commit: commit.clone(),
                artifact: None,
            })
        }
        RunSource::Artifact { artifact } => {
            let found = plan
                .artifacts
                .get(artifact)
                .ok_or_else(|| unsupported(format!("artifact {artifact} is not in the plan")))?;
            if !work.execution.artifact_ids.contains(artifact) {
                return Err(unsupported(format!(
                    "artifact {artifact} is not attached to {}",
                    work.key
                )));
            }
            let (asset, commit) = evidence(artifact.to_string(), found)?;
            repository(plan, work, asset)?;
            Ok(ExactSource {
                asset,
                commit,
                artifact: Some(*artifact),
            })
        }
    }
}

/// The asset and commit a `GitCommit` artifact consistently names.
fn evidence(label: String, found: &Artifact) -> Result<(AssetId, String), RunError> {
    if found.kind != ArtifactKind::GitCommit {
        return Err(unsupported(format!(
            "artifact {label} is not Git commit evidence"
        )));
    }
    let metadata = |name: &str| {
        found
            .metadata
            .get(name)
            .ok_or_else(|| unsupported(format!("artifact {label} has no `{name}` metadata")))
    };
    let commit = metadata("commit")?;
    if !is_exact_commit(commit) {
        return Err(unsupported(format!(
            "artifact {label} names {commit:?}, not a full commit; a branch or abbreviation is not an exact source"
        )));
    }
    let asset: AssetId = metadata("asset_id")?
        .parse()
        .map_err(|_| unsupported(format!("artifact {label} has an invalid `asset_id`")))?;
    if found.uri != format!("git:asset:{asset}@{commit}") {
        return Err(unsupported(format!(
            "artifact {label} locator {:?} disagrees with its commit and asset",
            found.uri
        )));
    }
    Ok((asset, commit.clone()))
}

/// The asset must be a Git repository the task's contract requires.
fn repository(plan: &Plan, work: &WorkItem, asset: AssetId) -> Result<(), RunError> {
    if !work.contract.assets.iter().any(|need| need.asset == asset) {
        return Err(unsupported(format!(
            "asset {asset} is not required by {}",
            work.key
        )));
    }
    match plan.assets.get(&asset).map(|found| &found.kind) {
        Some(AssetKind::GitRepository { .. }) => Ok(()),
        Some(_) => Err(unsupported(format!(
            "asset {asset} is not a Git repository"
        ))),
        None => Err(unsupported(format!("asset {asset} is not in the plan"))),
    }
}

fn unsupported(reason: String) -> RunError {
    RunError::UnsupportedSource(reason)
}
