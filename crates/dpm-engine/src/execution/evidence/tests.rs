use super::*;
use crate::{Command, admit_command, apply_command};
use chrono::Utc;
use dpm_model::{ActorId, ArtifactId, ArtifactKind, OperationId, Plan, WorkItemId};

fn claimed() -> (Plan, WorkItemId) {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    apply_command(
        &mut plan,
        ActorId::agent("owner"),
        Command::claim(work),
        Utc::now(),
        OperationId::new(),
    )
    .expect("claim");
    (plan, work)
}

fn evidence(kind: ArtifactKind, uri: &str, metadata: &[(&str, &str)]) -> Artifact {
    Artifact {
        id: ArtifactId::new(),
        kind,
        uri: uri.into(),
        label: "Acceptance run".into(),
        metadata: metadata
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        created_by: ActorId::agent("owner"),
        created_at: Utc::now(),
    }
}

const DIGEST: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
const FILE: [(&str, &str); 2] = [("sha256", DIGEST), ("bytes", "2048")];
const PASSED: [(&str, &str); 1] = [("outcome", "passed")];

fn attach(artifact: Artifact) -> Result<Plan, String> {
    let (mut plan, work) = claimed();
    let before = plan.clone();
    let command = Command::AttachArtifact { work, artifact };
    if let Err(error) = admit_command(&command) {
        return Err(error.to_string());
    }
    match apply_command(
        &mut plan,
        ActorId::agent("owner"),
        command,
        Utc::now(),
        OperationId::new(),
    ) {
        Ok(_) => Ok(plan),
        Err(error) => {
            assert_eq!(
                plan, before,
                "a rejected attachment leaves the plan unchanged"
            );
            Err(error.to_string())
        }
    }
}

#[test]
fn references_with_what_keeps_them_checkable_are_accepted() {
    use ArtifactKind::{Build, Cad, GitCommit, Other, Photo, PullRequest, Quote, TestResult};
    let fingerprinted = [FILE[0], FILE[1], ("outcome", "failed")];
    for (kind, uri, metadata) in [
        (
            GitCommit,
            "git:local@0123456789abcdef0123456789abcdef01234567",
            &[][..],
        ),
        (
            GitCommit,
            "git:asset:0192f000-0000-4000-8000-000000000001@0123456789abcdef",
            &[],
        ),
        (
            GitCommit,
            "git:git@github.com:org/repo.git@0123456789abcdef",
            &[],
        ),
        (
            Other,
            "git:0123456789abcdef0123456789abcdef01234567:docs/acceptance.md",
            &[],
        ),
        (PullRequest, "https://github.com/org/repo/pull/7", &[]),
        (TestResult, "https://ci.example.com/runs/42", &PASSED),
        (
            Build,
            "https://ci.example.com/runs/42/artifacts",
            &[("outcome", "partial")],
        ),
        (
            TestResult,
            "repo:target/acceptance/report.json",
            &fingerprinted,
        ),
        (Photo, "file:///Volumes/QA/inspection-17.jpg", &FILE),
        (Cad, "smb://fileserver/drawings/bracket-r3.step", &[]),
        (Quote, "urn:po:4711", &[]),
        (Other, "doi:10.1000/182", &[]),
    ] {
        let plan = attach(evidence(kind, uri, metadata)).unwrap_or_else(|e| panic!("{uri}: {e}"));
        assert!(plan.artifacts.values().any(|a| a.uri == uri), "{uri}");
    }
}

#[test]
fn content_store_copies_and_uncheckable_files_are_refused_with_guidance() {
    use ArtifactKind::{Other, Photo, TestResult};
    let bad_digest = [("sha256", "ABC"), ("bytes", "2048")];
    for (kind, uri, metadata, why) in [
        (
            Other,
            ".dpm/artifacts/run/tests.log",
            &[][..],
            "names no scheme",
        ),
        (Other, "target/report.txt", &[], "names no scheme"),
        (Other, "C:\\reports\\run.txt", &[], "names no scheme"),
        (
            Other,
            "data:image/png;base64,iVBORw0KGgo",
            &[],
            "carries content",
        ),
        (
            Other,
            "file:///Users/me/project/.dpm/artifacts/run.json",
            &FILE,
            "store directory",
        ),
        (
            Other,
            "repo:tools/.dpm/evidence.json",
            &FILE,
            "store directory",
        ),
        (
            Other,
            "git:0123456789abcdef:.dpm/run@1.json",
            &[],
            "store directory",
        ),
        (
            Other,
            "repo:/private/tmp/report.txt",
            &FILE,
            "leaves the repository",
        ),
        (
            Other,
            "repo:../elsewhere/report.txt",
            &FILE,
            "leaves the repository",
        ),
        (
            Photo,
            "repo:evidence/ui-run/screenshot.PNG",
            &FILE,
            "commits a screenshot",
        ),
        (
            Other,
            "git:0123456789abcdef:evidence/run@1/web-tests.wasm",
            &[],
            "commits a screenshot",
        ),
        (Photo, "file:///Volumes/QA/inspection-17.jpg", &[], "sha256"),
        (
            Other,
            "file:///Volumes/QA/report.txt",
            &bad_digest,
            "sha256",
        ),
        (TestResult, "https://ci.example.com/runs/42", &[], "outcome"),
        (
            TestResult,
            "https://ci.example.com/runs/42",
            &[("outcome", "green")],
            "outcome",
        ),
    ] {
        let error = attach(evidence(kind, uri, metadata)).expect_err(uri);
        assert!(error.contains(why), "{uri}: {error}");
        assert!(error.contains("attach_file"), "{uri}: {error}");
    }
}

#[test]
fn content_smuggled_through_long_fields_is_rejected() {
    let long = "x".repeat(MAX_EVIDENCE_URI_BYTES);
    let error = attach(evidence(
        ArtifactKind::Other,
        &format!("https://h/{long}"),
        &[],
    ))
    .expect_err("uri");
    assert!(error.contains("uri exceeds"), "{error}");

    let mut labelled = evidence(ArtifactKind::Other, "https://h/run", &[]);
    labelled.label = "x".repeat(MAX_EVIDENCE_LABEL_BYTES + 1);
    assert!(
        attach(labelled)
            .expect_err("label")
            .contains("label exceeds")
    );

    let mut valued = evidence(ArtifactKind::Other, "https://h/run", &[]);
    valued
        .metadata
        .insert("log".into(), "x".repeat(MAX_EVIDENCE_METADATA_BYTES + 1));
    assert!(attach(valued).expect_err("value").contains("at most"));

    let mut crowded = evidence(ArtifactKind::Other, "https://h/run", &[]);
    crowded.metadata = (0..=MAX_EVIDENCE_METADATA_ENTRIES)
        .map(|i| (format!("k{i}"), "v".into()))
        .collect();
    assert!(attach(crowded).expect_err("entries").contains("entries"));
}

#[test]
fn recorded_history_replays_evidence_the_rule_now_refuses() {
    let (mut plan, work) = claimed();
    let mut recorded = evidence(
        ArtifactKind::TestResult,
        ".dpm/artifacts/run/tests.log",
        &[],
    );
    recorded
        .metadata
        .insert("log".into(), "x".repeat(MAX_EVIDENCE_METADATA_BYTES + 1));
    let command = Command::AttachArtifact {
        work,
        artifact: recorded.clone(),
    };
    assert!(admit_command(&command).is_err(), "a new command is refused");
    apply_command(
        &mut plan,
        ActorId::agent("owner"),
        command,
        Utc::now(),
        OperationId::new(),
    )
    .expect("replay applies the recorded attachment");
    assert_eq!(plan.artifacts.get(&recorded.id), Some(&recorded));
}
