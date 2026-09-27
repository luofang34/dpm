use super::*;

fn link(
    work: WorkItemId,
    identity: ExternalIdentity,
    role: ExternalLinkRole,
    reference: ExternalReferenceId,
    observed: Option<ExternalState>,
) -> Command {
    let Command::LinkExternal(mut request) = request(work, identity, role) else {
        panic!("link request");
    };
    request.reference = reference;
    request.observed = observed;
    Command::LinkExternal(request)
}

#[test]
fn a_pull_request_link_upgrades_an_issue_record_of_the_same_number_and_never_downgrades() {
    let mut plan = fixture();
    let (a, b) = (id(&plan, "TEST-A"), id(&plan, "TEST-B"));
    let issue = forgejo("git.alpha.example", ExternalObjectKind::Issue);
    let pull = forgejo("git.alpha.example", ExternalObjectKind::PullRequest);
    run(
        &mut plan,
        "linker",
        request(a, issue.clone(), ExternalLinkRole::Tracks),
    )
    .expect("issue link");
    let recorded = reference_of(&plan, &issue);
    let tracks = ExternalLinkRole::Tracks;
    let merged = Some(ExternalState::Merged);
    // Merged is refused while the record is an issue and the request names one.
    rejected(&mut plan, link(a, issue.clone(), tracks, recorded, merged));
    // A role change is not an upgrade, and another work cannot take tracking through one.
    let related = link(a, pull.clone(), ExternalLinkRole::Relates, recorded, None);
    assert!(matches!(
        rejected(&mut plan, related),
        EngineError::DuplicateExternalLink { .. }
    ));
    assert!(matches!(
        rejected(&mut plan, link(b, pull.clone(), tracks, recorded, None)),
        EngineError::TrackingOwned { .. }
    ));

    let operation = run(
        &mut plan,
        "worker",
        link(a, pull.clone(), tracks, recorded, merged),
    )
    .expect("the owner relinks the same number as a merged pull request");
    let Command::LinkExternal(logged) = &operation.command else {
        panic!("link operation");
    };
    assert_eq!(logged.identity.kind, ExternalObjectKind::PullRequest);
    let record = &plan.external_references[&recorded];
    assert_eq!(record.identity, pull, "one record, now a pull request");
    assert_eq!(record.links.len(), 1);
    assert_eq!(record.tracking_owner(), Some(a));
    assert_eq!(
        record.observation.as_ref().map(|o| o.state),
        Some(ExternalState::Merged)
    );
    assert_eq!(plan.external_references.len(), 1);

    let closed = Some(ExternalState::Closed);
    let context = link(
        b,
        issue.clone(),
        ExternalLinkRole::Relates,
        recorded,
        closed,
    );
    run(&mut plan, "observer", context).expect("issue-kind context link");
    assert_eq!(
        plan.external_references[&recorded].identity.kind,
        ExternalObjectKind::PullRequest,
        "an issue-kind request never downgrades the record"
    );
    assert!(matches!(
        rejected(&mut plan, link(a, pull, tracks, recorded, merged)),
        EngineError::DuplicateExternalLink { .. }
    ));
}

#[test]
fn separately_numbered_forges_keep_issues_and_merge_requests_apart() {
    let mut plan = fixture();
    let a = id(&plan, "TEST-A");
    let gitlab = |kind| ExternalIdentity {
        provider: ExternalProvider::GitLab,
        ..forgejo("gitlab.example", kind)
    };
    run(
        &mut plan,
        "linker",
        request(
            a,
            gitlab(ExternalObjectKind::Issue),
            ExternalLinkRole::Tracks,
        ),
    )
    .expect("issue");
    let merge_request = gitlab(ExternalObjectKind::PullRequest);
    run(
        &mut plan,
        "linker",
        request(a, merge_request.clone(), ExternalLinkRole::Tracks),
    )
    .expect("a merge request with the same number is another object");
    assert_eq!(plan.external_references.len(), 2);
    let issue = reference_of(&plan, &gitlab(ExternalObjectKind::Issue));
    assert_eq!(
        plan.external_references[&issue].identity.kind,
        ExternalObjectKind::Issue
    );
}
