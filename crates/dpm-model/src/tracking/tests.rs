use super::*;
use crate::{Key, Plan};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn work(plan: &Plan, key: &str) -> WorkItemId {
    plan.find_work_by_key(key).expect("work").id
}

fn identity(provider: ExternalProvider, instance: &str, namespace: &str) -> ExternalIdentity {
    ExternalIdentity {
        provider,
        instance: instance.into(),
        namespace: Some(namespace.into()),
        kind: ExternalObjectKind::Issue,
        external_id: "42".into(),
    }
}

fn reference(
    identity: ExternalIdentity,
    work: WorkItemId,
    role: ExternalLinkRole,
) -> ExternalReference {
    ExternalReference {
        id: ExternalReferenceId::new(),
        label: format!("Tracked {identity}"),
        identity,
        url: None,
        observation: None,
        links: BTreeSet::from([ExternalLink { work, role }]),
    }
}

fn insert(plan: &mut Plan, reference: ExternalReference) -> ExternalReferenceId {
    let id = reference.id;
    plan.external_references.insert(id, reference);
    id
}

fn reason(plan: &Plan) -> String {
    plan.validate().expect_err("invalid").to_string()
}

#[test]
fn equal_numbers_from_separate_instances_namespaces_and_kinds_do_not_collide() {
    let mut plan = fixture();
    let ids = ["TEST-A", "TEST-B", "TEST-C", "TEST-D"].map(|key| work(&plan, key));
    let a = identity(ExternalProvider::Forgejo, "git.alpha.example", "ops/dpm");
    let b = identity(
        ExternalProvider::Forgejo,
        "git.beta.example:3000",
        "ops/dpm",
    );
    let c = identity(ExternalProvider::Forgejo, "git.alpha.example", "ops/other");
    let mut d = a.clone();
    d.kind = ExternalObjectKind::PullRequest;
    for (identity, work) in [a, b, c, d].into_iter().zip(ids) {
        insert(
            &mut plan,
            reference(identity, work, ExternalLinkRole::Tracks),
        );
    }
    plan.validate().expect("distinct identities");
    let mut hosted = identity(ExternalProvider::GitHub, "github.com", "ops/dpm");
    hosted.external_id = "42".into();
    let e = work(&plan, "TEST-E");
    insert(&mut plan, reference(hosted, e, ExternalLinkRole::Tracks));
    plan.validate()
        .expect("hosted and self-hosted numbering are separate");
}

#[test]
fn one_identity_has_one_record_and_one_tracking_owner() {
    let mut plan = fixture();
    let (a, b) = (work(&plan, "TEST-A"), work(&plan, "TEST-B"));
    let shared = identity(ExternalProvider::GitHub, "github.com", "ops/dpm");
    let id = insert(
        &mut plan,
        reference(shared.clone(), a, ExternalLinkRole::Tracks),
    );
    let duplicate = insert(&mut plan, reference(shared, b, ExternalLinkRole::Relates));
    assert!(reason(&plan).contains("already recorded"));
    plan.external_references.remove(&duplicate);
    let links = &mut plan.external_references.get_mut(&id).expect("ref").links;
    links.insert(ExternalLink {
        work: b,
        role: ExternalLinkRole::Relates,
    });
    plan.validate().expect("related context may be shared");
    let links = &mut plan.external_references.get_mut(&id).expect("ref").links;
    links.insert(ExternalLink {
        work: b,
        role: ExternalLinkRole::Tracks,
    });
    assert!(reason(&plan).contains("linked more than once"));
    links_of(&mut plan, id).retain(|l| l.work != b);
    let c = work(&plan, "TEST-C");
    links_of(&mut plan, id).insert(ExternalLink {
        work: c,
        role: ExternalLinkRole::Tracks,
    });
    assert!(reason(&plan).contains("2 tracking owners"));
}

fn links_of(plan: &mut Plan, id: ExternalReferenceId) -> &mut BTreeSet<ExternalLink> {
    &mut plan.external_references.get_mut(&id).expect("ref").links
}

#[test]
fn canonical_spelling_is_required_so_case_cannot_split_identity() {
    let loose = ExternalIdentity {
        provider: ExternalProvider::Other("GitHub".into()),
        instance: " https://GitHub.com/ ".into(),
        namespace: Some("/Owner/Repo/".into()),
        kind: ExternalObjectKind::Other("PR".into()),
        external_id: "#7".into(),
    };
    let canonical = loose.canonical();
    assert_eq!(canonical.provider, ExternalProvider::GitHub);
    assert_eq!(canonical.instance, "github.com");
    assert_eq!(canonical.namespace.as_deref(), Some("owner/repo"));
    assert_eq!(canonical.kind, ExternalObjectKind::PullRequest);
    assert_eq!(canonical.external_id, "7");
    assert_eq!(
        canonical.to_string(),
        "GitHub:github.com/owner/repo:pull_request:7"
    );
    let mut plan = fixture();
    let a = work(&plan, "TEST-A");
    insert(&mut plan, reference(loose, a, ExternalLinkRole::Tracks));
    assert!(reason(&plan).contains("not canonical"));
    let jira = ExternalIdentity {
        provider: ExternalProvider::Jira,
        instance: "jira.corp.example".into(),
        namespace: Some("OPS".into()),
        kind: ExternalObjectKind::Issue,
        external_id: "OPS-12".into(),
    };
    assert_eq!(jira.canonical(), jira, "Jira keys keep their case");
}

#[test]
fn provider_rules_require_namespaces_and_valid_kinds() {
    let base = fixture();
    let a = work(&base, "TEST-A");
    let check = |identity: ExternalIdentity| {
        let mut plan = base.clone();
        insert(&mut plan, reference(identity, a, ExternalLinkRole::Tracks));
        plan.validate().map_err(|e| e.to_string())
    };
    let mut forge = identity(
        ExternalProvider::GitLab,
        "gitlab.example",
        "group/sub/project",
    );
    check(forge.clone()).expect("nested GitLab groups");
    forge.namespace = None;
    assert!(check(forge).expect_err("namespace").contains("namespace"));
    let mut jira = identity(ExternalProvider::Jira, "acme.atlassian.net", "OPS");
    jira.namespace = None;
    jira.external_id = "OPS-7".into();
    check(jira.clone()).expect("Jira keys are instance-scoped");
    jira.kind = ExternalObjectKind::PullRequest;
    assert!(check(jira).expect_err("kind").contains("no pull requests"));
    let mut bad_id = identity(ExternalProvider::GitHub, "github.com", "o/r");
    bad_id.external_id = "4 2".into();
    assert!(check(bad_id).is_err());
    let custom = ExternalIdentity {
        provider: ExternalProvider::Other("redmine".into()),
        kind: ExternalObjectKind::Other("ticket".into()),
        ..identity(ExternalProvider::GitHub, "redmine.example", "ops")
    };
    check(custom).expect("other providers");
}

#[test]
fn credentials_are_rejected_in_every_shared_field() {
    let base = fixture();
    let a = work(&base, "TEST-A");
    let github = identity(ExternalProvider::GitHub, "github.com", "ops/dpm");
    let check = |edit: &dyn Fn(&mut ExternalReference)| {
        let mut plan = base.clone();
        let mut value = reference(github.clone(), a, ExternalLinkRole::Tracks);
        edit(&mut value);
        insert(&mut plan, value);
        plan.validate().map_err(|e| e.to_string())
    };
    check(&|r| r.url = Some("https://github.com/ops/dpm/issues/42".into())).expect("plain URL");
    check(&|r| {
        r.url = Some("https://github.com/ops/dpm/issues/42?author=me#issuecomment-1".into())
    })
    .expect("ordinary parameters");
    for url in [
        "https://ghp_secret@github.com/ops/dpm/issues/42",
        "https://user:pass@github.com/ops/dpm/issues/42",
        "https://github.com/ops/dpm/issues/42?access_token=abc",
        "https://github.com/ops/dpm/issues/42?private_token=abc",
        "https://github.com/ops/dpm/issues/42#token=abc",
        "https://github.com/ops/dpm/issues/42?X-Amz-Signature=abc",
    ] {
        let error = check(&|r| r.url = Some(url.into())).expect_err(url);
        assert!(
            error.contains("credentials") || error.contains("secret"),
            "{url}: {error}"
        );
    }
    let error = check(&|r| r.url = Some("https://evil.example/ops/dpm/issues/42".into()))
        .expect_err("foreign host");
    assert!(error.contains("match the identity instance"));
    assert!(check(&|r| r.url = Some("ftp://github.com/x".into())).is_err());
    let error =
        check(&|r| r.label = "see https://tok@github.com/ops/dpm".into()).expect_err("label");
    assert!(error.contains("credentials"));
    let error = check(&|r| r.identity.instance = "tok@github.com".into()).expect_err("instance");
    assert!(error.contains("credentials"));
    let error =
        check(&|r| r.identity.namespace = Some("user:pw@ops".into())).expect_err("namespace");
    assert!(error.contains("namespace"));
}

#[test]
fn links_need_existing_work_and_merged_state_needs_a_pull_request() {
    let base = fixture();
    let a = work(&base, "TEST-A");
    let github = identity(ExternalProvider::GitHub, "github.com", "ops/dpm");
    let mut plan = base.clone();
    insert(
        &mut plan,
        reference(github.clone(), WorkItemId::new(), ExternalLinkRole::Tracks),
    );
    assert!(reason(&plan).contains("missing work"));
    let mut plan = base.clone();
    let mut empty = reference(github.clone(), a, ExternalLinkRole::Tracks);
    empty.links.clear();
    insert(&mut plan, empty);
    assert!(reason(&plan).contains("at least one work link"));
    let mut plan = base.clone();
    let mut merged = reference(github, a, ExternalLinkRole::Tracks);
    merged.observation = Some(ExternalObservation {
        state: ExternalState::Merged,
        observed_at: Utc::now(),
        observed_by: crate::ActorId::agent("observer"),
    });
    let id = insert(&mut plan, merged);
    assert!(reason(&plan).contains("only pull requests"));
    plan.external_references
        .get_mut(&id)
        .expect("ref")
        .identity
        .kind = ExternalObjectKind::PullRequest;
    plan.validate().expect("merged pull request observation");
}

#[test]
fn references_are_additive_and_survive_renames_and_serialization() {
    let mut plan = fixture();
    let before = serde_json::to_value(&plan).expect("json");
    assert!(
        before.get("external_references").is_none(),
        "empty field is omitted"
    );
    let a = work(&plan, "TEST-A");
    let id = insert(
        &mut plan,
        reference(
            identity(ExternalProvider::GitHub, "github.com", "ops/dpm"),
            a,
            ExternalLinkRole::Tracks,
        ),
    );
    plan.find_work_by_key_mut("TEST-A").expect("work").key = Key::new("TEST-A-RENAMED");
    let entry = plan.external_references.get_mut(&id).expect("ref");
    entry.label = "Renamed display".into();
    entry.identity.namespace = Some("ops/dpm-renamed".into());
    plan.validate()
        .expect("identity is independent of keys and labels");
    let text = serde_json::to_string(&plan).expect("json");
    let loaded: Plan = serde_json::from_str(&text).expect("round trip");
    assert_eq!(loaded, plan);
    assert_eq!(loaded.external_references[&id].tracking_owner(), Some(a));
}

#[test]
fn one_jira_issue_has_one_identity_whatever_its_spelling() {
    let jira = |namespace: Option<&str>, key: &str| ExternalIdentity {
        provider: ExternalProvider::Jira,
        instance: "acme.atlassian.net".into(),
        namespace: namespace.map(Into::into),
        kind: ExternalObjectKind::Issue,
        external_id: key.into(),
    };
    let canonical = jira(None, "PROJ-1");
    assert_eq!(jira(None, "proj-1").canonical(), canonical);
    assert_eq!(jira(None, " Proj-1 ").canonical(), canonical);
    let mut plan = fixture();
    let (a, b) = (work(&plan, "TEST-A"), work(&plan, "TEST-B"));
    insert(&mut plan, reference(canonical, a, ExternalLinkRole::Tracks));
    plan.validate().expect("canonical Jira identity");
    let scoped = insert(
        &mut plan,
        reference(jira(Some("PROJ"), "PROJ-1"), b, ExternalLinkRole::Tracks),
    );
    assert!(reason(&plan).contains("omit the namespace"));
    plan.external_references.remove(&scoped);
    insert(
        &mut plan,
        reference(jira(None, "proj-1"), b, ExternalLinkRole::Relates),
    );
    assert!(reason(&plan).contains("not canonical"));
}

#[test]
fn default_ports_do_not_split_an_instance() {
    let with_port = identity(ExternalProvider::GitHub, "github.com:443", "ops/dpm");
    let plain = identity(ExternalProvider::GitHub, "github.com", "ops/dpm");
    assert_eq!(with_port.canonical(), plain);
    let http = identity(ExternalProvider::Forgejo, "forge.example:80", "ops/dpm");
    assert_eq!(http.canonical().instance, "forge.example");
    let custom = identity(ExternalProvider::Forgejo, "forge.example:3000", "ops/dpm");
    assert_eq!(custom.canonical().instance, "forge.example:3000");
}
