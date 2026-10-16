use super::*;
use crate::{Command, admit_command, apply_command};
use chrono::Utc;
use dpm_model::{ActorId, ArtifactId, ArtifactKind, OperationId, Plan, WorkItemId};
use std::collections::BTreeMap;

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

fn evidence(uri: &str) -> Artifact {
    Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::TestResult,
        uri: uri.into(),
        label: "Acceptance run".into(),
        metadata: BTreeMap::new(),
        created_by: ActorId::agent("owner"),
        created_at: Utc::now(),
    }
}

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
fn commits_hosted_pages_and_committed_text_files_are_accepted() {
    for uri in [
        "git:local@0123456789abcdef0123456789abcdef01234567",
        "git:asset:0192f000-0000-4000-8000-000000000001@0123456789abcdef",
        "git:git@github.com:org/repo.git@0123456789abcdef",
        "git:0123456789abcdef0123456789abcdef01234567:docs/acceptance.md",
        "repo:target/acceptance/report.json",
        "https://github.com/org/repo/pull/7",
        "https://ci.example.com/runs/42/screenshot.png",
        "urn:po:4711",
        "doi:10.1000/182",
    ] {
        let plan = attach(evidence(uri)).unwrap_or_else(|e| panic!("{uri}: {e}"));
        assert!(plan.artifacts.values().any(|a| a.uri == uri), "{uri}");
    }
}

#[test]
fn local_copies_and_embedded_content_are_rejected_with_guidance() {
    for (uri, why) in [
        (".dpm/artifacts/run/tests.log", "names no scheme"),
        ("target/report.txt", "names no scheme"),
        ("file:report.txt", "scheme \"file\""),
        ("file:///results/a.txt", "scheme \"file\""),
        ("data:image/png;base64,iVBORw0KGgo", "scheme \"data\""),
        (
            "repo:.dpm/artifacts/run/tests.log",
            "project store directory",
        ),
        ("repo:tools/.dpm/evidence.json", "project store directory"),
        (
            "git:0123456789abcdef:.dpm/evidence/run.json",
            "project store directory",
        ),
        ("repo:/private/tmp/report.txt", "leaves the repository"),
        ("repo:../elsewhere/report.txt", "leaves the repository"),
        ("repo:evidence/ui-run/screenshot.PNG", "screenshot"),
        ("repo:evidence/web-tests.wasm", "screenshot"),
        ("git:0123456789abcdef:evidence/capture.mov", "screenshot"),
        (
            "git:0123456789abcdef:.dpm/run@1.json",
            "project store directory",
        ),
        (
            "git:0123456789abcdef:evidence/run@1/capture.png",
            "screenshot",
        ),
    ] {
        let error = attach(evidence(uri)).expect_err(uri);
        assert!(error.contains(why), "{uri}: {error}");
        assert!(error.contains("attach_git_head"), "{uri}: {error}");
    }
}

#[test]
fn content_smuggled_through_long_fields_is_rejected() {
    let long = "x".repeat(MAX_EVIDENCE_URI_BYTES);
    let error = attach(evidence(&format!("https://h/{long}"))).expect_err("uri");
    assert!(error.contains("uri exceeds"), "{error}");

    let mut labelled = evidence("https://h/run");
    labelled.label = "x".repeat(MAX_EVIDENCE_LABEL_BYTES + 1);
    assert!(
        attach(labelled)
            .expect_err("label")
            .contains("label exceeds")
    );

    let mut valued = evidence("https://h/run");
    valued
        .metadata
        .insert("log".into(), "x".repeat(MAX_EVIDENCE_METADATA_BYTES + 1));
    assert!(attach(valued).expect_err("value").contains("at most"));

    let mut crowded = evidence("https://h/run");
    crowded.metadata = (0..=MAX_EVIDENCE_METADATA_ENTRIES)
        .map(|i| (format!("k{i}"), "v".into()))
        .collect();
    assert!(attach(crowded).expect_err("entries").contains("entries"));
}

#[test]
fn recorded_history_replays_evidence_the_rule_now_refuses() {
    let (mut plan, work) = claimed();
    let mut recorded = evidence(".dpm/artifacts/run/tests.log");
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
