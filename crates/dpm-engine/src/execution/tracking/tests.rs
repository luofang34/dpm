use crate::{
    Command, EngineError, ExternalLinkRequest, NextWorkQuery, apply_command, explain_work,
    next_work, propose_change, status,
};
use chrono::Utc;
use dpm_model::{
    ActorId, ExternalIdentity, ExternalLinkRole, ExternalObjectKind, ExternalProvider,
    ExternalReferenceId, ExternalState, Key, Plan, WorkItem, WorkItemId, WorkKind, WorkStatus,
};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn id(plan: &Plan, key: &str) -> WorkItemId {
    plan.find_work_by_key(key).expect("work").id
}

fn forgejo(instance: &str, kind: ExternalObjectKind) -> ExternalIdentity {
    ExternalIdentity {
        provider: ExternalProvider::Forgejo,
        instance: instance.into(),
        namespace: Some("ops/dpm".into()),
        kind,
        external_id: "42".into(),
    }
}

fn request(work: WorkItemId, identity: ExternalIdentity, role: ExternalLinkRole) -> Command {
    Command::LinkExternal(ExternalLinkRequest {
        work,
        reference: ExternalReferenceId::new(),
        label: format!("Tracked {identity}"),
        identity,
        url: None,
        role,
        observed: None,
    })
}

fn run(plan: &mut Plan, actor: &str, command: Command) -> Result<crate::Operation, EngineError> {
    apply_command(
        plan,
        ActorId::agent(actor),
        command,
        Utc::now(),
        dpm_model::OperationId::new(),
    )
}

/// A rejected command must leave every field, including the revision, byte-identical.
fn rejected(plan: &mut Plan, command: Command) -> EngineError {
    let before = serde_json::to_string(&*plan).expect("json");
    let error = run(plan, "linker", command).expect_err("rejected");
    assert_eq!(serde_json::to_string(&*plan).expect("json"), before);
    error
}

fn reference_of(plan: &Plan, identity: &ExternalIdentity) -> ExternalReferenceId {
    plan.external_references
        .values()
        .find(|r| r.identity == *identity)
        .expect("recorded")
        .id
}

#[test]
fn equal_ids_from_separate_self_hosted_instances_are_tracked_independently() {
    let mut plan = fixture();
    let (a, b, c) = (
        id(&plan, "TEST-A"),
        id(&plan, "TEST-B"),
        id(&plan, "TEST-C"),
    );
    let alpha = forgejo("git.alpha.example", ExternalObjectKind::Issue);
    let beta = forgejo("git.beta.example", ExternalObjectKind::Issue);
    let merge = ExternalIdentity {
        external_id: "43".into(),
        ..forgejo("git.alpha.example", ExternalObjectKind::PullRequest)
    };
    run(
        &mut plan,
        "linker",
        request(a, alpha.clone(), ExternalLinkRole::Tracks),
    )
    .expect("alpha");
    run(
        &mut plan,
        "linker",
        request(b, beta.clone(), ExternalLinkRole::Tracks),
    )
    .expect("beta");
    run(
        &mut plan,
        "linker",
        request(c, merge, ExternalLinkRole::Tracks),
    )
    .expect("pull request");
    assert_eq!(plan.external_references.len(), 3);
    assert_eq!(plan.revision, 3);
    let owner =
        |i: &ExternalIdentity| plan.external_references[&reference_of(&plan, i)].tracking_owner();
    assert_eq!(owner(&alpha), Some(a));
    assert_eq!(owner(&beta), Some(b));
    // Forgejo numbers issues and pull requests together, so PR #42 is issue #42.
    let same_number = forgejo("git.alpha.example", ExternalObjectKind::PullRequest);
    let before = plan.clone();
    run(
        &mut plan,
        "linker",
        request(c, same_number, ExternalLinkRole::Tracks),
    )
    .expect_err("one object, one reference");
    assert_eq!(plan, before);
}

#[test]
fn tracking_ownership_is_exclusive_and_failed_links_change_nothing() {
    let mut plan = fixture();
    let (a, b) = (id(&plan, "TEST-A"), id(&plan, "TEST-B"));
    let issue = forgejo("git.alpha.example", ExternalObjectKind::Issue);
    run(
        &mut plan,
        "linker",
        request(a, issue.clone(), ExternalLinkRole::Tracks),
    )
    .expect("link");
    let recorded = reference_of(&plan, &issue);
    let reuse = |work, role| {
        let Command::LinkExternal(mut r) = request(work, issue.clone(), role) else {
            panic!("link request");
        };
        r.reference = recorded;
        Command::LinkExternal(r)
    };
    let error = rejected(&mut plan, reuse(b, ExternalLinkRole::Tracks));
    assert!(matches!(&error, EngineError::TrackingOwned { owner, .. } if owner.0 == "TEST-A"));
    assert!(matches!(
        rejected(&mut plan, reuse(a, ExternalLinkRole::Relates)),
        EngineError::DuplicateExternalLink { .. }
    ));
    let error = rejected(
        &mut plan,
        request(b, issue.clone(), ExternalLinkRole::Relates),
    );
    assert!(error.to_string().contains("already recorded"), "{error}");
    assert!(matches!(
        rejected(
            &mut plan,
            request(WorkItemId::new(), issue.clone(), ExternalLinkRole::Relates)
        ),
        EngineError::MissingWorkItem(_)
    ));
    let Command::LinkExternal(mut rewrite) = request(
        b,
        forgejo("x.example", ExternalObjectKind::Issue),
        ExternalLinkRole::Tracks,
    ) else {
        panic!("link request");
    };
    rewrite.reference = recorded;
    let error = rejected(&mut plan, Command::LinkExternal(rewrite));
    assert!(error.to_string().contains("cannot be rewritten"), "{error}");
    let Command::LinkExternal(mut secret) = request(
        b,
        forgejo("git.beta.example", ExternalObjectKind::Issue),
        ExternalLinkRole::Tracks,
    ) else {
        panic!("link request");
    };
    secret.url = Some("https://git.beta.example/ops/dpm/issues/42?access_token=abc".into());
    assert!(matches!(
        rejected(&mut plan, Command::LinkExternal(secret)),
        EngineError::Validation(_)
    ));
    run(&mut plan, "linker", reuse(b, ExternalLinkRole::Relates)).expect("related context");
    assert_eq!(plan.external_references[&recorded].links.len(), 2);
}

#[test]
fn unlinking_removes_only_the_link_and_never_the_graph() {
    let mut plan = fixture();
    let (a, b) = (id(&plan, "TEST-A"), id(&plan, "TEST-B"));
    let issue = forgejo("git.alpha.example", ExternalObjectKind::Issue);
    let graph = (
        plan.work_items.clone(),
        plan.dependencies.clone(),
        plan.artifacts.clone(),
    );
    run(
        &mut plan,
        "linker",
        request(a, issue.clone(), ExternalLinkRole::Tracks),
    )
    .expect("link");
    let reference = reference_of(&plan, &issue);
    let Command::LinkExternal(mut related) = request(b, issue, ExternalLinkRole::Relates) else {
        panic!("link request");
    };
    related.reference = reference;
    run(&mut plan, "linker", Command::LinkExternal(related)).expect("relate");
    let unlink = |work| Command::UnlinkExternal { work, reference };
    run(&mut plan, "other", unlink(a)).expect("unlink owner");
    assert_eq!(plan.external_references[&reference].tracking_owner(), None);
    assert!(matches!(
        rejected(&mut plan, unlink(a)),
        EngineError::MissingExternalLink { .. }
    ));
    run(&mut plan, "other", unlink(b)).expect("unlink last");
    assert!(
        plan.external_references.is_empty(),
        "last unlink removes the record"
    );
    assert_eq!(
        (
            plan.work_items.clone(),
            plan.dependencies.clone(),
            plan.artifacts.clone()
        ),
        graph
    );
    assert!(matches!(
        rejected(&mut plan, unlink(b)),
        EngineError::MissingExternalReference(_)
    ));
    assert_eq!(plan.revision, 4);
}

/// Derived views at one clock reading with the revision normalized, plus the plan without its
/// references. The clock is fixed because started work's remaining forecast shrinks as it runs.
fn projections(plan: &Plan, now: chrono::DateTime<chrono::Utc>) -> String {
    let mut current = plan.clone();
    current.revision = 0;
    let query = NextWorkQuery {
        capabilities: Default::default(),
        use_probabilistic_criticality: false,
    };
    let next = next_work(&current, &query, now).expect("next");
    let status = status(&current, false, now).expect("status");
    let schedule = dpm_schedule::deterministic_remaining(&current, now).expect("schedule");
    current.external_references.clear();
    serde_json::to_string(&(next, status, schedule, current)).expect("json")
}

#[test]
fn external_state_is_an_observation_and_never_verification_or_evidence() {
    let mut plan = fixture();
    let (a, b) = (id(&plan, "TEST-A"), id(&plan, "TEST-B"));
    let before = chrono::Utc::now();
    let baseline = projections(&plan, before);
    let Command::LinkExternal(mut closed) = request(
        b,
        forgejo("git.alpha.example", ExternalObjectKind::Issue),
        ExternalLinkRole::Tracks,
    ) else {
        panic!("link request");
    };
    closed.observed = Some(ExternalState::Closed);
    run(&mut plan, "observer", Command::LinkExternal(closed)).expect("closed issue");
    assert_eq!(
        projections(&plan, before),
        baseline,
        "next, status and schedule are unchanged"
    );
    assert_eq!(plan.work_items[&b].execution.status, WorkStatus::Planned);

    run(&mut plan, "worker", Command::Claim { work: a }).expect("claim");
    run(&mut plan, "worker", Command::Start { work: a }).expect("start");
    run(
        &mut plan,
        "worker",
        Command::Submit {
            work: a,
            note: None,
        },
    )
    .expect("submit");
    let observed = chrono::Utc::now();
    let submitted = projections(&plan, observed);
    let Command::LinkExternal(mut merged) = request(
        a,
        ExternalIdentity {
            external_id: "7".into(),
            ..forgejo("git.alpha.example", ExternalObjectKind::PullRequest)
        },
        ExternalLinkRole::Tracks,
    ) else {
        panic!("link request");
    };
    merged.observed = Some(ExternalState::Merged);
    run(&mut plan, "worker", Command::LinkExternal(merged)).expect("merged pull request");
    assert_eq!(projections(&plan, observed), submitted);
    let task = &plan.work_items[&a];
    assert_eq!(
        task.execution.status,
        WorkStatus::Submitted,
        "merge is not verification"
    );
    assert!(
        task.execution.artifact_ids.is_empty(),
        "a tracking link is not evidence"
    );
    assert!(plan.artifacts.is_empty());
    assert!(matches!(
        run(
            &mut plan,
            "worker",
            Command::Verify {
                work: a,
                note: None
            }
        ),
        Err(EngineError::SelfVerification(_))
    ));
    let explained = explain_work(&plan, a, chrono::Utc::now()).expect("explain");
    assert!(explained.context.artifacts.is_empty());
    let observation = explained.context.external_references[0]
        .observation
        .as_ref()
        .expect("observed");
    assert_eq!(observation.state, ExternalState::Merged);
    assert_eq!(observation.observed_by, ActorId::agent("worker"));
}

#[test]
fn explain_exposes_package_references_and_survives_reviewed_renames() {
    let mut plan = fixture();
    let a = id(&plan, "TEST-A");
    let mut package: WorkItem = plan.work_items[&a].clone();
    package.id = WorkItemId::new();
    package.key = Key::new("TEST-PKG");
    package.kind = WorkKind::WorkPackage;
    package.contract.acceptance.clear();
    package.contract.instructions = None;
    package.schedule.estimate = None;
    package.execution.status = WorkStatus::Planned;
    plan.work_items.insert(package.id, package.clone());
    plan.work_items.get_mut(&a).expect("task").parent = Some(package.id);
    plan.validate().expect("package");
    // GitLab numbers epics apart from issues; shared-number forges have no such kind.
    let epic = ExternalIdentity {
        provider: ExternalProvider::GitLab,
        ..forgejo(
            "gitlab.alpha.example",
            ExternalObjectKind::Other("epic".into()),
        )
    };
    run(
        &mut plan,
        "linker",
        request(package.id, epic.clone(), ExternalLinkRole::Tracks),
    )
    .expect("epic");
    let reference = reference_of(&plan, &epic);
    let context = explain_work(&plan, a, chrono::Utc::now())
        .expect("explain")
        .context;
    assert_eq!(context.external_references[0].id, reference);
    assert!(
        explain_work(&plan, id(&plan, "TEST-B"), chrono::Utc::now())
            .expect("other")
            .context
            .external_references
            .is_empty()
    );

    let mut proposal = plan.clone();
    let record = proposal
        .external_references
        .get_mut(&reference)
        .expect("ref");
    record.label = "Moved epic".into();
    record.identity.namespace = Some("platform/dpm".into());
    let preview = propose_change(&plan, &proposal).expect("preview");
    let change = preview
        .changes
        .iter()
        .find(|c| c.collection == "external_references")
        .expect("reference diff");
    assert_eq!(change.fields, ["identity", "label"]);
    crate::apply_plan_change(
        &mut plan,
        ActorId::human("reviewer"),
        &proposal,
        "Repository moved",
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("apply");
    let moved = explain_work(&plan, a, chrono::Utc::now())
        .expect("explain")
        .context
        .external_references;
    assert_eq!(
        (moved[0].id, moved[0].tracking_owner()),
        (reference, Some(package.id))
    );
    run(
        &mut plan,
        "linker",
        Command::UnlinkExternal {
            work: package.id,
            reference,
        },
    )
    .expect("unlink by id");
}

#[test]
fn reviewed_changes_cannot_author_or_rewrite_observations() {
    let mut plan = fixture();
    let (a, b) = (id(&plan, "TEST-A"), id(&plan, "TEST-B"));
    let Command::LinkExternal(mut closed) = request(
        a,
        forgejo("git.alpha.example", ExternalObjectKind::Issue),
        ExternalLinkRole::Tracks,
    ) else {
        panic!("link request");
    };
    closed.observed = Some(ExternalState::Closed);
    let recorded = closed.reference;
    run(&mut plan, "observer", Command::LinkExternal(closed)).expect("observed link");
    let mut rewritten = plan.clone();
    let observation = rewritten
        .external_references
        .get_mut(&recorded)
        .and_then(|r| r.observation.as_mut())
        .expect("observation");
    observation.state = ExternalState::Open;
    observation.observed_by = ActorId::human("someone-else");
    let error = propose_change(&plan, &rewritten).expect_err("rewrite");
    assert!(
        error
            .to_string()
            .contains("observations are recorded by link"),
        "{error}"
    );
    let mut authored = plan.clone();
    let mut added = authored.external_references[&recorded].clone();
    added.id = ExternalReferenceId::new();
    added.identity = forgejo("git.beta.example", ExternalObjectKind::Issue);
    added.links = [dpm_model::ExternalLink {
        work: b,
        role: ExternalLinkRole::Relates,
    }]
    .into();
    authored.external_references.insert(added.id, added.clone());
    assert!(
        propose_change(&plan, &authored).is_err(),
        "new reference with an observation"
    );
    added.observation = None;
    authored.external_references.insert(added.id, added);
    propose_change(&plan, &authored).expect("reviewed reference without observation");
}

mod generated;
mod kind;
mod neighbours;
mod rules;
