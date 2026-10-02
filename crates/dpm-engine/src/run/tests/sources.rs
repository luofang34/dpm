//! Source references resolve to exact commits the plan supports, or are refused.

use super::*;
use dpm_model::{Artifact, ArtifactId, AssetId, ExactSource};
use std::collections::BTreeMap;

fn asset(plan: &Plan) -> AssetId {
    *plan.assets.keys().next().expect("asset")
}

fn evidence(asset: AssetId, commit: &str) -> Artifact {
    Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::GitCommit,
        uri: format!("git:asset:{asset}@{commit}"),
        label: "HEAD".into(),
        metadata: BTreeMap::from([
            ("commit".to_string(), commit.to_string()),
            ("asset_id".to_string(), asset.to_string()),
        ]),
        created_by: worker(),
        created_at: Utc::now(),
    }
}

/// Attach `artifact` to the claimed task the way `attach-git-head` does.
fn attach(plan: &mut Plan, work: WorkItemId, artifact: &Artifact) {
    apply_command(
        plan,
        worker(),
        Command::AttachArtifact {
            work,
            artifact: artifact.clone(),
        },
        Utc::now(),
        OperationId::new(),
    )
    .expect("attach");
}

fn start_with(plan: &Plan, work: WorkItemId, source: RunSource) -> Result<RunRecord, RunError> {
    let mut run = request(work);
    run.sources = vec![source];
    observe_start(plan, LineageId::new(), &worker(), &run, Utc::now())
}

fn refused(result: Result<RunRecord, RunError>, needle: &str) {
    match result {
        Err(RunError::UnsupportedSource(reason)) => {
            assert!(
                reason.contains(needle),
                "{reason:?} should mention {needle:?}"
            )
        }
        other => panic!("expected an unsupported source mentioning {needle:?}, got {other:?}"),
    }
}

#[test]
fn a_named_commit_must_belong_to_a_repository_the_task_requires_and_is_captured_exactly() {
    let (plan, work) = claimed();
    let asset = asset(&plan);
    let commit = "c".repeat(40);
    let record = start_with(
        &plan,
        work,
        RunSource::GitCommit {
            asset,
            commit: commit.clone(),
        },
    )
    .expect("required repository");
    assert_eq!(
        record.exact_sources,
        [ExactSource {
            asset,
            commit: commit.clone(),
            artifact: None
        }]
    );
    refused(
        start_with(
            &plan,
            work,
            RunSource::GitCommit {
                asset: AssetId::new(),
                commit,
            },
        ),
        "not required",
    );
}

#[test]
fn attached_evidence_is_captured_as_the_exact_commit_it_names() {
    let (mut plan, work) = claimed();
    let asset = asset(&plan);
    let commit = "d".repeat(40);
    let artifact = evidence(asset, &commit);
    attach(&mut plan, work, &artifact);
    let record = start_with(
        &plan,
        work,
        RunSource::Artifact {
            artifact: artifact.id,
        },
    )
    .expect("attached evidence");
    assert_eq!(
        record.exact_sources,
        [ExactSource {
            asset,
            commit,
            artifact: Some(artifact.id)
        }]
    );
    assert_eq!(
        record.sources,
        [RunSource::Artifact {
            artifact: artifact.id
        }]
    );
}

#[test]
fn a_git_artifact_naming_a_mutable_branch_is_not_an_exact_source() {
    let (mut plan, work) = claimed();
    let branch = Artifact {
        uri: "git:local@main".into(),
        metadata: BTreeMap::new(),
        ..evidence(asset(&plan), &"e".repeat(40))
    };
    attach(&mut plan, work, &branch);
    refused(
        start_with(
            &plan,
            work,
            RunSource::Artifact {
                artifact: branch.id,
            },
        ),
        "commit",
    );
    // Metadata that names a branch or an abbreviation instead of a full commit is refused too.
    for named in ["main", "abc1234", &"E".repeat(40)] {
        let mut weak = evidence(asset(&plan), &"e".repeat(40));
        weak.metadata.insert("commit".into(), named.to_string());
        attach(&mut plan, work, &weak);
        refused(
            start_with(&plan, work, RunSource::Artifact { artifact: weak.id }),
            "not a full commit",
        );
    }
}

#[test]
fn evidence_must_agree_with_itself_be_attached_to_the_task_and_name_a_required_repository() {
    let (mut plan, work) = claimed();
    let asset = asset(&plan);
    let commit = "f".repeat(40);

    let mut elsewhere = evidence(asset, &commit);
    elsewhere.uri = format!("git:asset:{asset}@{}", "0".repeat(40));
    attach(&mut plan, work, &elsewhere);
    refused(
        start_with(
            &plan,
            work,
            RunSource::Artifact {
                artifact: elsewhere.id,
            },
        ),
        "disagrees",
    );

    let mut other_asset = evidence(asset, &commit);
    other_asset
        .metadata
        .insert("asset_id".into(), AssetId::new().to_string());
    attach(&mut plan, work, &other_asset);
    refused(
        start_with(
            &plan,
            work,
            RunSource::Artifact {
                artifact: other_asset.id,
            },
        ),
        "disagrees",
    );

    let stranger = evidence(AssetId::new(), &commit);
    attach(&mut plan, work, &stranger);
    refused(
        start_with(
            &plan,
            work,
            RunSource::Artifact {
                artifact: stranger.id,
            },
        ),
        "not required",
    );

    // Evidence in the plan that is not attached to this task says nothing about this task.
    let unattached = evidence(asset, &commit);
    plan.artifacts.insert(unattached.id, unattached.clone());
    refused(
        start_with(
            &plan,
            work,
            RunSource::Artifact {
                artifact: unattached.id,
            },
        ),
        "not attached",
    );

    let mut report = evidence(asset, &commit);
    report.kind = ArtifactKind::TestResult;
    attach(&mut plan, work, &report);
    refused(
        start_with(
            &plan,
            work,
            RunSource::Artifact {
                artifact: report.id,
            },
        ),
        "not Git commit evidence",
    );
    refused(
        start_with(
            &plan,
            work,
            RunSource::Artifact {
                artifact: ArtifactId::new(),
            },
        ),
        "not in the plan",
    );
}
