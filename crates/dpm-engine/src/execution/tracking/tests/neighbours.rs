//! Linking and reviewed plan changes apply one object identity: kind synonyms cannot take a second
//! tracking owner, and removing a record while adding one for the same object is an edit of that
//! object, so it cannot drop a `Merged` observation or downgrade a pull request.

use super::rules::linked;
use super::*;

fn object(
    provider: ExternalProvider,
    namespace: Option<&str>,
    kind: &str,
    id: &str,
) -> ExternalIdentity {
    ExternalIdentity {
        provider,
        instance: "tracker.example".into(),
        namespace: namespace.map(Into::into),
        kind: ExternalObjectKind::Other(kind.into()),
        external_id: id.into(),
    }
    .canonical()
}

/// The first kind is tracked by one work item; every other kind is a second owner of that object.
fn second_owner_refused(spellings: &[ExternalIdentity]) {
    let mut plan = fixture();
    let (a, b) = (id(&plan, "TEST-A"), id(&plan, "TEST-B"));
    let tracks = ExternalLinkRole::Tracks;
    linked(&mut plan, a, spellings[0].clone(), tracks, None).expect("owner");
    for spelling in &spellings[1..] {
        let error = linked(&mut plan, b, spelling.clone(), tracks, None)
            .expect_err("one object has one tracking owner");
        assert!(
            matches!(error, EngineError::TrackingOwned { .. }),
            "{spelling}: {error}"
        );
    }
    assert_eq!(plan.external_references.len(), 1);
}

#[test]
fn issue_types_and_aliases_cannot_take_a_second_owner() {
    let gitlab = |kind| object(ExternalProvider::GitLab, Some("ops/dpm"), kind, "6");
    second_owner_refused(&[gitlab("epic"), gitlab("epics")]);
    second_owner_refused(&[gitlab("issue"), gitlab("incident"), gitlab("work_item")]);
    let jira = |kind, key| object(ExternalProvider::Jira, None, kind, key);
    second_owner_refused(&[
        jira("issue", "PROJ-6"),
        jira("bug", "PROJ-6"),
        jira("Story", "PROJ-6"),
        jira("issue", "PROJ-06"),
    ]);
    let linear = |kind| object(ExternalProvider::Linear, Some("acme"), kind, "ENG-6");
    second_owner_refused(&[linear("issue"), linear("task")]);
}

#[test]
fn gitlab_number_spaces_stay_apart() {
    let mut plan = fixture();
    let tracks = ExternalLinkRole::Tracks;
    for (work, kind) in [
        ("TEST-A", "issue"),
        ("TEST-B", "merge_request"),
        ("TEST-C", "epic"),
    ] {
        let identity = object(ExternalProvider::GitLab, Some("ops/dpm"), kind, "6");
        let work = id(&plan, work);
        linked(&mut plan, work, identity, tracks, None).expect(kind);
    }
    assert_eq!(plan.external_references.len(), 3);
}

/// Replace the only record with a new record id for `identity`, keeping its links, and review it.
fn rekeyed(
    plan: &Plan,
    identity: ExternalIdentity,
    keep_observation: bool,
) -> Result<(), EngineError> {
    let mut proposal = plan.clone();
    let mut records = std::mem::take(&mut proposal.external_references).into_values();
    let (Some(mut new), None) = (records.next(), records.next()) else {
        panic!("one reference");
    };
    new.id = ExternalReferenceId::new();
    new.identity = identity;
    if !keep_observation {
        new.observation = None;
    }
    proposal.external_references.insert(new.id, new);
    let preview = propose_change(plan, &proposal).map(|_| ());
    // Applying is the reviewed path that mutates; it must decide exactly as the preview does.
    let mut applied = plan.clone();
    let result = apply_command(
        &mut applied,
        ActorId::human("reviewer"),
        Command::ApplyChange {
            plan: Box::new(proposal),
            reason: "re-add the object".into(),
        },
        Utc::now(),
    );
    assert_eq!(preview.is_ok(), result.is_ok(), "preview and apply agree");
    if result.is_err() {
        assert_eq!(&applied, plan, "a refused apply changes nothing");
    }
    result.map(|_| ())
}

fn github(kind: ExternalObjectKind) -> ExternalIdentity {
    ExternalIdentity {
        provider: ExternalProvider::GitHub,
        instance: "github.com".into(),
        namespace: Some("o/r".into()),
        kind,
        external_id: "5".into(),
    }
}

#[test]
fn a_rekeyed_pull_request_is_not_downgraded_in_review() {
    let mut plan = fixture();
    let a = id(&plan, "TEST-A");
    let pull = github(ExternalObjectKind::PullRequest);
    linked(&mut plan, a, pull, ExternalLinkRole::Tracks, None).expect("pull request");
    for kind in [
        ExternalObjectKind::Issue,
        ExternalObjectKind::Other("discussion".into()),
    ] {
        let error = rekeyed(&plan, github(kind), true).expect_err("downgrade through a re-key");
        assert!(error.to_string().contains("kind"), "{error}");
    }
    rekeyed(&plan, github(ExternalObjectKind::PullRequest), true)
        .expect("an unchanged re-key keeps the object as it is");
}

#[test]
fn a_rekeyed_merged_pull_request_keeps_its_observation_in_review() {
    let mut plan = fixture();
    let a = id(&plan, "TEST-A");
    let pull = github(ExternalObjectKind::PullRequest);
    let merged = Some(ExternalState::Merged);
    linked(&mut plan, a, pull.clone(), ExternalLinkRole::Tracks, merged).expect("merged");
    let spelled = ExternalIdentity {
        external_id: "#05".into(),
        instance: "GitHub.com:443".into(),
        ..pull.clone()
    };
    for identity in [pull.clone(), spelled.canonical()] {
        let error = rekeyed(&plan, identity, false).expect_err("dropping Merged through a re-key");
        assert!(error.to_string().contains("observation"), "{error}");
    }
    let error = rekeyed(&plan, github(ExternalObjectKind::Issue), false)
        .expect_err("dropping Merged and downgrading through a re-key");
    assert!(error.to_string().contains("kind"), "{error}");
    rekeyed(&plan, pull, true).expect("an unchanged re-key keeps the object as it is");
}

#[test]
fn a_rekey_may_refine_or_relabel_within_one_number_space() {
    let mut plan = fixture();
    let a = id(&plan, "TEST-A");
    let issue = github(ExternalObjectKind::Issue);
    linked(&mut plan, a, issue, ExternalLinkRole::Tracks, None).expect("issue");
    rekeyed(&plan, github(ExternalObjectKind::PullRequest), true).expect("refinement by re-key");
    let jira = |kind| object(ExternalProvider::Jira, None, kind, "PROJ-6");
    let mut plan = fixture();
    linked(&mut plan, a, jira("bug"), ExternalLinkRole::Tracks, None).expect("bug");
    rekeyed(&plan, jira("story"), true).expect("a Jira issue type is a label of one key");
}

#[test]
fn the_owner_restates_a_kind_only_within_its_number_space() {
    let mut plan = fixture();
    let a = id(&plan, "TEST-A");
    let tracks = ExternalLinkRole::Tracks;
    let gitlab = |kind| object(ExternalProvider::GitLab, Some("ops/dpm"), kind, "6");
    linked(&mut plan, a, gitlab("issue"), tracks, None).expect("issue");
    linked(&mut plan, a, gitlab("incident"), tracks, None).expect("an issue becomes an incident");
    let [record] = plan.external_references.values().collect::<Vec<_>>()[..] else {
        panic!("one reference");
    };
    assert_eq!(record.identity, gitlab("incident"));
    let mut plan = fixture();
    let pull = github(ExternalObjectKind::PullRequest);
    linked(&mut plan, a, pull.clone(), tracks, None).expect("pull request");
    let error = linked(
        &mut plan,
        a,
        github(ExternalObjectKind::Issue),
        tracks,
        None,
    )
    .expect_err("a pull request is not restated as an issue");
    assert!(
        matches!(error, EngineError::DuplicateExternalLink { .. }),
        "{error}"
    );
    let [record] = plan.external_references.values().collect::<Vec<_>>()[..] else {
        panic!("one reference");
    };
    assert_eq!(record.identity, pull);
}
