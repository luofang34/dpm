use super::*;
use crate::{ActorId, AssetId, RunEventId, RunId, WorkItemId};

fn start() -> RunStart {
    RunStart {
        id: RunId::new(),
        work: WorkItemId::new(),
        executor: ActorId::agent("worker"),
        parent: None,
        session: Some(RunSession {
            provider: "codex".into(),
            session: "thread-1".into(),
            turn: Some("turn-4".into()),
        }),
        observation: Observation::ReportedOnly,
        sources: vec![RunSource::GitCommit {
            asset: AssetId::new(),
            commit: "a".repeat(40),
        }],
        observed_at: None,
    }
}

#[test]
fn a_well_formed_start_is_accepted_and_round_trips_through_json() {
    let request = start();
    request.validate().expect("valid");
    let json = serde_json::to_string(&request).expect("json");
    assert_eq!(
        serde_json::from_str::<RunStart>(&json).expect("decode"),
        request
    );
}

#[test]
fn run_and_event_identities_must_be_version_7() {
    let mut request = start();
    request.id = RunId(uuid::Uuid::new_v4());
    assert!(request.validate().is_err());
    let transition = RunTransition {
        id: RunEventId(uuid::Uuid::new_v4()),
        run: RunId::new(),
        to: RunState::Waiting,
        detail: None,
        observed_at: None,
    };
    assert!(transition.validate().is_err());
}

#[test]
fn a_source_must_name_an_exact_commit_not_a_branch_or_abbreviation() {
    for commit in ["main", "abc1234", &"A".repeat(40), &"a".repeat(41), ""] {
        let source = RunSource::GitCommit {
            asset: AssetId::new(),
            commit: commit.into(),
        };
        assert!(source.validate().is_err(), "{commit:?}");
    }
    for length in [40, 64] {
        let source = RunSource::GitCommit {
            asset: AssetId::new(),
            commit: "0".repeat(length),
        };
        source.validate().expect("full object name");
    }
}

#[test]
fn session_identifiers_are_bounded_opaque_text_with_a_lowercase_provider() {
    let mut session = start().session.expect("session");
    session.validate().expect("valid");
    for provider in ["", "Codex", "co dex", "1codex", &"a".repeat(33)] {
        session.provider = provider.into();
        assert!(session.validate().is_err(), "{provider:?}");
    }
    session.provider = "codex".into();
    session.session = "line\nbreak".into();
    assert!(session.validate().is_err());
}

#[test]
fn a_run_cannot_be_its_own_parent_or_carry_unbounded_sources() {
    let mut request = start();
    request.parent = Some(request.id);
    assert!(request.validate().is_err());
    request.parent = None;
    request.sources = (0..=MAX_SOURCES)
        .map(|_| RunSource::Artifact {
            artifact: crate::ArtifactId::new(),
        })
        .collect();
    assert!(request.validate().is_err());
}

#[test]
fn states_parse_from_their_words_and_only_three_are_terminal() {
    for state in [
        RunState::Working,
        RunState::Waiting,
        RunState::Failed,
        RunState::Interrupted,
        RunState::Completed,
    ] {
        assert_eq!(state.word().parse::<RunState>(), Ok(state));
        assert_eq!(
            serde_json::to_value(state).expect("json"),
            serde_json::Value::String(state.word().into())
        );
    }
    assert!("done".parse::<RunState>().is_err());
    let terminal: Vec<_> = [
        RunState::Working,
        RunState::Waiting,
        RunState::Failed,
        RunState::Interrupted,
        RunState::Completed,
    ]
    .into_iter()
    .filter(|state| state.is_terminal())
    .collect();
    assert_eq!(
        terminal,
        [RunState::Failed, RunState::Interrupted, RunState::Completed]
    );
}

#[test]
fn activity_text_is_cut_at_a_character_boundary_and_flagged() {
    let mut input = ActivityInput {
        run: RunId::new(),
        source_sequence: 1,
        kind: ActivityKind::ToolResult,
        text: Some("é".repeat(MAX_ACTIVITY_TEXT_BYTES)),
        observed_at: None,
    };
    let normalized = input.normalized();
    assert!(normalized.truncated);
    let kept = normalized.text.clone().expect("text");
    assert!(kept.len() <= MAX_ACTIVITY_TEXT_BYTES && kept.chars().all(|c| c == 'é'));
    // A retry of the same input normalizes identically, which is what idempotency compares.
    assert_eq!(input.normalized(), normalized);
    input.text = Some("short".into());
    assert!(!input.normalized().truncated);
    input.text = None;
    assert_eq!(input.normalized().text, None);
    assert_eq!(input.normalized().text_digest, None);
}

#[test]
fn the_digest_covers_the_whole_text_so_a_changed_tail_beyond_the_kept_prefix_differs() {
    let prefix = "x".repeat(MAX_ACTIVITY_TEXT_BYTES);
    let input = |tail: &str| ActivityInput {
        run: RunId::new(),
        source_sequence: 1,
        kind: ActivityKind::ToolResult,
        text: Some(format!("{prefix}{tail}")),
        observed_at: None,
    };
    let (a, b) = (input("A").normalized(), input("B").normalized());
    assert_eq!(a.text, b.text, "the kept prefix is the same");
    assert!(a.truncated && b.truncated);
    assert_ne!(a.text_digest, b.text_digest);
    assert_eq!(a.text_digest, input("A").normalized().text_digest);
    assert_eq!(a.text_digest.as_deref().map(str::len), Some(64));
}

#[test]
fn a_source_sequence_starts_at_one() {
    let mut input = ActivityInput {
        run: RunId::new(),
        source_sequence: 1,
        kind: ActivityKind::Heartbeat,
        text: None,
        observed_at: None,
    };
    input.validate().expect("valid");
    input.source_sequence = u64::MAX;
    input
        .validate()
        .expect("the largest sequence is a valid, final one");
    input.source_sequence = 0;
    assert!(input.validate().is_err());
}

#[test]
fn kind_and_observation_words_fail_with_typed_errors_naming_the_word() {
    assert_eq!(
        "tool_started".parse::<ActivityKind>(),
        Ok(ActivityKind::ToolStarted)
    );
    let error = "telemetry"
        .parse::<ActivityKind>()
        .expect_err("unknown kind");
    assert!(error.to_string().contains("telemetry"));
    let error = "observed".parse::<Observation>().expect_err("unknown mode");
    assert!(error.to_string().contains("observed"));
}

#[test]
fn observation_modes_parse_from_their_words() {
    assert_eq!("managed".parse::<Observation>(), Ok(Observation::Managed));
    assert_eq!(
        "reported_only".parse::<Observation>(),
        Ok(Observation::ReportedOnly)
    );
    assert!("observed".parse::<Observation>().is_err());
}
