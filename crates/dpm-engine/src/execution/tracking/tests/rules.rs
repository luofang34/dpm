//! Kind refinement, observation finality and identity folding hold on every path.

use super::*;

fn github(kind: ExternalObjectKind) -> ExternalIdentity {
    ExternalIdentity {
        provider: ExternalProvider::GitHub,
        instance: "github.com".into(),
        namespace: Some("o/r".into()),
        kind,
        external_id: "5".into(),
    }
}

/// Link as the application does: reuse the reference already recorded for the object.
pub(super) fn linked(
    plan: &mut Plan,
    work: WorkItemId,
    identity: ExternalIdentity,
    role: ExternalLinkRole,
    observed: Option<ExternalState>,
) -> Result<crate::Operation, EngineError> {
    let Command::LinkExternal(mut request) = request(work, identity, role) else {
        panic!("link request");
    };
    request.observed = observed;
    if let Some(recorded) = plan
        .external_references
        .values()
        .find(|r| r.identity.object_key() == request.identity.object_key())
    {
        request.reference = recorded.id;
    }
    let before = serde_json::to_string(&*plan).expect("json");
    let result = run(plan, "linker", Command::LinkExternal(request));
    if result.is_err() {
        assert_eq!(serde_json::to_string(&*plan).expect("json"), before);
    }
    result
}

fn only_kind(plan: &Plan) -> ExternalObjectKind {
    let [reference] = plan.external_references.values().collect::<Vec<_>>()[..] else {
        panic!("one reference");
    };
    reference.identity.kind.clone()
}

#[test]
fn only_the_tracking_owner_refines_the_recorded_kind() {
    let mut plan = fixture();
    let (a, b, c) = (
        id(&plan, "TEST-A"),
        id(&plan, "TEST-B"),
        id(&plan, "TEST-C"),
    );
    let (issue, pull) = (
        github(ExternalObjectKind::Issue),
        github(ExternalObjectKind::PullRequest),
    );
    let (tracks, relates) = (ExternalLinkRole::Tracks, ExternalLinkRole::Relates);
    linked(&mut plan, a, issue.clone(), tracks, None).expect("owner");
    linked(&mut plan, b, pull.clone(), relates, None).expect("context link");
    assert_eq!(
        only_kind(&plan),
        ExternalObjectKind::Issue,
        "context never refines"
    );
    let merged = Some(ExternalState::Merged);
    linked(&mut plan, c, pull.clone(), relates, merged).expect_err("issue record is not merged");
    linked(&mut plan, a, pull, tracks, merged).expect("the owner refines the kind");
    assert_eq!(only_kind(&plan), ExternalObjectKind::PullRequest);
}

#[test]
fn a_record_without_an_owner_is_not_refined_by_context_links() {
    let mut plan = fixture();
    let (a, b) = (id(&plan, "TEST-A"), id(&plan, "TEST-B"));
    let relates = ExternalLinkRole::Relates;
    linked(
        &mut plan,
        a,
        github(ExternalObjectKind::Issue),
        relates,
        None,
    )
    .expect("context");
    linked(
        &mut plan,
        b,
        github(ExternalObjectKind::PullRequest),
        relates,
        None,
    )
    .expect("context");
    assert_eq!(only_kind(&plan), ExternalObjectKind::Issue);
}

#[test]
fn a_merged_observation_is_final_for_every_link() {
    let mut plan = fixture();
    let (a, b, c) = (
        id(&plan, "TEST-A"),
        id(&plan, "TEST-B"),
        id(&plan, "TEST-C"),
    );
    let pull = github(ExternalObjectKind::PullRequest);
    let relates = ExternalLinkRole::Relates;
    let merged = Some(ExternalState::Merged);
    linked(&mut plan, a, pull.clone(), ExternalLinkRole::Tracks, merged).expect("merged");
    for (work, kind, state) in [
        (b, ExternalObjectKind::Issue, ExternalState::Closed),
        (b, ExternalObjectKind::PullRequest, ExternalState::Open),
        (c, ExternalObjectKind::Issue, ExternalState::Open),
    ] {
        let error = linked(&mut plan, work, github(kind), relates, Some(state))
            .expect_err("merged is final");
        assert!(error.to_string().contains("Merged"), "{error}");
    }
    linked(&mut plan, b, pull, relates, merged).expect("a repeated merged report");
    let [reference] = plan.external_references.values().collect::<Vec<_>>()[..] else {
        panic!("one reference");
    };
    assert_eq!(
        reference.observation.as_ref().map(|o| o.state),
        Some(ExternalState::Merged)
    );
}

fn reviewed_kind_change(
    identity: ExternalIdentity,
    to: ExternalObjectKind,
) -> Result<(), EngineError> {
    let mut plan = fixture();
    let a = id(&plan, "TEST-A");
    let tracks = ExternalLinkRole::Tracks;
    linked(&mut plan, a, identity, tracks, None).expect("record");
    let mut proposal = plan.clone();
    for reference in proposal.external_references.values_mut() {
        reference.identity.kind = to.clone();
    }
    propose_change(&plan, &proposal).map(|_| ())
}

#[test]
fn reviewed_changes_follow_the_link_kind_rule() {
    let issue = ExternalObjectKind::Issue;
    let pull = ExternalObjectKind::PullRequest;
    let error = reviewed_kind_change(github(pull.clone()), issue.clone())
        .expect_err("review cannot downgrade a pull request");
    assert!(error.to_string().contains("kind"), "{error}");
    reviewed_kind_change(github(issue.clone()), pull.clone()).expect("review may refine");
    let gitlab = ExternalIdentity {
        provider: ExternalProvider::GitLab,
        ..github(issue)
    };
    reviewed_kind_change(gitlab, pull).expect_err("a GitLab merge request is another object");
}

const KINDS: &[&str] = &[
    "Issue",
    "issues",
    "PullRequest",
    "pull_requests",
    "pull-requests",
    "prs",
    "pullrequests",
    "pulls",
    "discussion",
];
const INSTANCES: &[&str] = &[
    "github.com",
    "github.com:443",
    "github.com:0443",
    "GitHub.com.",
    "https://github.com:00443/",
];
const NAMESPACES: &[&str] = &["o/r", "./o/r", "o//r", "/o/./r/", "O/R.git"];

#[test]
fn generated_spellings_of_one_object_have_one_tracking_owner() {
    let mut plan = fixture();
    let (a, b) = (id(&plan, "TEST-A"), id(&plan, "TEST-B"));
    let tracks = ExternalLinkRole::Tracks;
    linked(
        &mut plan,
        a,
        github(ExternalObjectKind::Issue),
        tracks,
        None,
    )
    .expect("owner");
    for kind in KINDS {
        for kind in [
            ExternalObjectKind::parse(kind),
            ExternalObjectKind::Other((*kind).into()),
        ] {
            for instance in INSTANCES {
                for namespace in NAMESPACES {
                    let spelling = ExternalIdentity {
                        instance: (*instance).into(),
                        namespace: Some((*namespace).into()),
                        ..github(kind.clone())
                    }
                    .canonical();
                    let error = linked(&mut plan, b, spelling.clone(), tracks, None)
                        .expect_err("second owner");
                    assert!(
                        matches!(error, EngineError::TrackingOwned { .. }),
                        "{spelling:?}: {error}"
                    );
                }
            }
        }
    }
    assert_eq!(plan.external_references.len(), 1);
}

#[test]
fn requests_are_validated_even_when_the_object_is_recorded() {
    let mut plan = fixture();
    let (a, b) = (id(&plan, "TEST-A"), id(&plan, "TEST-B"));
    let relates = ExternalLinkRole::Relates;
    linked(
        &mut plan,
        a,
        github(ExternalObjectKind::Issue),
        relates,
        None,
    )
    .expect("record");
    for kind in ["weird", "pull_requests"] {
        let spelled = github(ExternalObjectKind::Other(kind.into()));
        linked(&mut plan, b, spelled, relates, None).expect_err(kind);
    }
    assert_eq!(plan.external_references.len(), 1);
}
