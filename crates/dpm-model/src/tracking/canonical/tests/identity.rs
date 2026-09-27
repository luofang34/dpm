//! Generated identity spellings: every spelling of one object must reach one object key.

use crate::{ExternalIdentity, ExternalObjectKind, ExternalProvider, ExternalReferenceId};

const ISSUE_KINDS: &[&str] = &["Issue", "issues", "ISSUE", " Issues "];
const REVIEW_KINDS: &[&str] = &[
    "PullRequest",
    "pull_requests",
    "pull-requests",
    "prs",
    "pullrequests",
    "Pull Requests",
    "pulls",
    "pr",
    "merge_requests",
    "MR",
    "mrs",
];
const DISCUSSION_KINDS: &[&str] = &["discussion", "Discussions"];
const PORTS: &[&str] = &[
    "git.example",
    "git.example:443",
    "git.example:0443",
    "git.example:80",
    "git.example:00080",
    "Git.Example.",
    "https://git.example:443/",
];
const NAMESPACE_FORMS: &[&str] = &[
    "ops/dpm",
    "./ops/dpm",
    "ops/./dpm",
    "ops//dpm",
    "/ops/dpm/",
    "Ops/Dpm.git",
    "./ops/dpm/.",
];

/// Canonical identities for every spelling of one object, with the kind given both through the
/// adapter parser and as a raw `Other` name, as JSON callers send it.
fn object_spellings(provider: &ExternalProvider, kinds: &[&str]) -> Vec<ExternalIdentity> {
    let mut all = Vec::new();
    for kind in kinds {
        for parsed in [
            ExternalObjectKind::parse(kind),
            ExternalObjectKind::Other((*kind).into()),
        ] {
            for instance in PORTS {
                for namespace in NAMESPACE_FORMS {
                    for external_id in ["42", "#042"] {
                        all.push(
                            ExternalIdentity {
                                provider: provider.clone(),
                                instance: (*instance).into(),
                                namespace: Some((*namespace).into()),
                                kind: parsed.clone(),
                                external_id: external_id.into(),
                            }
                            .canonical(),
                        );
                    }
                }
            }
        }
    }
    all
}

/// Every spelling is valid and every spelling shares one object key.
fn one_key(spellings: &[ExternalIdentity]) -> crate::ObjectKey {
    let key = spellings[0].object_key();
    for spelling in spellings {
        spelling
            .validate(ExternalReferenceId::new())
            .unwrap_or_else(|error| panic!("{spelling:?}: {error}"));
        assert_eq!(spelling.object_key(), key, "{spelling:?} splits {key:?}");
    }
    key
}

#[test]
fn generated_kind_port_and_namespace_spellings_name_one_object_per_provider() {
    let shared = [
        ExternalProvider::GitHub,
        ExternalProvider::Forgejo,
        ExternalProvider::Gitea,
    ];
    let mut forge_keys = Vec::new();
    for provider in &shared {
        let mut spellings = object_spellings(provider, ISSUE_KINDS);
        spellings.extend(object_spellings(provider, REVIEW_KINDS));
        if *provider == ExternalProvider::GitHub {
            // GitHub numbers discussions in the issue and pull request sequence.
            spellings.extend(object_spellings(provider, DISCUSSION_KINDS));
        }
        forge_keys.push(one_key(&spellings));
    }
    assert_eq!(
        forge_keys[1], forge_keys[2],
        "Forgejo and Gitea are one family"
    );
    assert_ne!(forge_keys[0], forge_keys[1]);
    let gitlab = ExternalProvider::GitLab;
    let issue = one_key(&object_spellings(&gitlab, ISSUE_KINDS));
    let merge_request = one_key(&object_spellings(&gitlab, REVIEW_KINDS));
    assert_ne!(
        issue, merge_request,
        "GitLab numbers merge requests separately"
    );
}

fn base(provider: ExternalProvider, kind: &str) -> ExternalIdentity {
    ExternalIdentity {
        provider,
        instance: "git.example".into(),
        namespace: Some("ops/dpm".into()),
        kind: ExternalObjectKind::parse(kind),
        external_id: "42".into(),
    }
}

#[test]
fn shared_number_forges_reject_kinds_outside_their_numbered_set() {
    for provider in [
        ExternalProvider::GitHub,
        ExternalProvider::Forgejo,
        ExternalProvider::Gitea,
    ] {
        for kind in ["weird", "pull_request_review", "ticket"] {
            let identity = base(provider.clone(), kind).canonical();
            assert!(
                identity.validate(ExternalReferenceId::new()).is_err(),
                "{identity:?}"
            );
        }
    }
    for provider in [ExternalProvider::Forgejo, ExternalProvider::Gitea] {
        let identity = base(provider, "discussion").canonical();
        assert!(identity.validate(ExternalReferenceId::new()).is_err());
    }
    let epic = base(ExternalProvider::GitLab, "epic").canonical();
    epic.validate(ExternalReferenceId::new())
        .expect("GitLab keeps its other object kinds");
}

#[test]
fn parent_segments_and_out_of_range_ports_are_rejected() {
    let github = base(ExternalProvider::GitHub, "issue");
    for namespace in ["ops/../dpm", "../ops/dpm", "ops/dpm/.."] {
        let identity = ExternalIdentity {
            namespace: Some(namespace.into()),
            ..github.clone()
        }
        .canonical();
        assert!(
            identity.validate(ExternalReferenceId::new()).is_err(),
            "{namespace}"
        );
    }
    for instance in ["git.example:0", "git.example:65536", "git.example:000"] {
        let identity = ExternalIdentity {
            instance: instance.into(),
            ..github.clone()
        }
        .canonical();
        assert!(
            identity.validate(ExternalReferenceId::new()).is_err(),
            "{instance}"
        );
    }
}
