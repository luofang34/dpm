//! Generated cross-product over the provider table: providers × number spaces × kind aliases ×
//! identifier spellings × instance and port spellings × namespace spellings. Every spelling of one
//! object validates and shares one key; objects in different number spaces never share a key; a
//! second record for any spelling of a recorded object is rejected.

use super::fold_kind;
use crate::tracking::tests::{fixture, insert, reference, work};
use crate::{
    ExternalIdentity, ExternalLinkRole, ExternalObjectKind, ExternalProvider, ExternalReferenceId,
    ObjectKey,
};

/// Expected number spaces of one provider, each with the kind spellings that must land in it.
struct Case {
    provider: ExternalProvider,
    instances: &'static [&'static str],
    namespaces: &'static [Option<&'static str>],
    ids: &'static [&'static str],
    spaces: &'static [&'static [&'static str]],
}

const FORGE_HOSTS: &[&str] = &[
    "git.example",
    "Git.Example",
    "git.example:443",
    "git.example:0443",
    "https://git.example/",
    "git.example.",
    "HTTP://git.example:80/",
];
const GITHUB_HOSTS: &[&str] = &[
    "github.com",
    "GitHub.com",
    "github.com:443",
    "https://www.github.com/",
    "www.github.com.",
    "WWW.GitHub.com:0443",
];
const REPOSITORIES: &[Option<&str>] = &[
    Some("o/r"),
    Some("O/R"),
    Some("./o//r/"),
    Some("o/r.git"),
    Some("O/R.Git/"),
];
const NUMBERS: &[&str] = &["6", "#6", "006", "#006", " 6 "];
const ISSUE_SEQUENCE: &[&str] = &[
    "Issue",
    "issues",
    "ISSUE",
    "PullRequest",
    "pull_requests",
    "pull-requests",
    "Pull Requests",
    "pulls",
    "prs",
    "PR",
    "merge_request",
    "MRs",
];

const GITHUB_SEQUENCE: &[&str] = &[
    "Issue",
    "issues",
    "PullRequest",
    "Pull Requests",
    "pulls",
    "prs",
    "MRs",
    "discussion",
    "Discussions",
];
const GITLAB_SPACES: &[&[&str]] = &[
    &[
        "Issue",
        "issues",
        "incident",
        "Incidents",
        "task",
        "Tasks",
        "test_case",
        "Test Cases",
        "ticket",
        "objective",
        "key_result",
        "Key Results",
        "work_item",
        "Work Items",
        "WorkItems",
    ],
    &[
        "PullRequest",
        "merge_requests",
        "MR",
        "mrs",
        "Merge Request",
        "pulls",
    ],
    &["epic", "epics", "Epic", "EPICS"],
];
const JIRA_TYPES: &[&str] = &[
    "Issue",
    "issues",
    "bug",
    "Bugs",
    "Story",
    "stories",
    "Task",
    "Sub-task",
    "Sub-Tasks",
    "epic",
    "Custom Type",
];

fn cases() -> Vec<Case> {
    let forge = |provider| Case {
        provider,
        instances: FORGE_HOSTS,
        namespaces: REPOSITORIES,
        ids: NUMBERS,
        spaces: &[ISSUE_SEQUENCE],
    };
    vec![
        Case {
            provider: ExternalProvider::GitHub,
            instances: GITHUB_HOSTS,
            namespaces: REPOSITORIES,
            ids: NUMBERS,
            spaces: &[GITHUB_SEQUENCE],
        },
        forge(ExternalProvider::Forgejo),
        forge(ExternalProvider::Gitea),
        Case {
            provider: ExternalProvider::GitLab,
            instances: FORGE_HOSTS,
            namespaces: REPOSITORIES,
            ids: &["6", "#6", "!6", "&6", "#006"],
            spaces: GITLAB_SPACES,
        },
        Case {
            provider: ExternalProvider::Jira,
            instances: FORGE_HOSTS,
            namespaces: &[None],
            ids: &["PROJ-6", "proj-6", "Proj-06", "#PROJ-006", " proj-6 "],
            spaces: &[JIRA_TYPES],
        },
        Case {
            provider: ExternalProvider::Linear,
            instances: FORGE_HOSTS,
            namespaces: &[Some("acme"), Some("Acme"), Some("/acme/")],
            ids: &["ENG-6", "eng-6", "ENG-06", "#eng-006"],
            spaces: &[&["Issue", "issues", "task", "Sub-issue", "bug"]],
        },
        Case {
            provider: ExternalProvider::Other("custom".into()),
            instances: FORGE_HOSTS,
            namespaces: &[Some("o/r"), Some("./o//r/")],
            ids: &["6", "#6", "!6"],
            spaces: &[
                &["Issue", "issues"],
                &["PullRequest", "pulls", "prs"],
                &["widget", "Widgets", "WIDGET"],
                &["gadget", "Gadgets"],
            ],
        },
    ]
}

/// Every spelling of one object in `kinds`, with the kind given both parsed and raw.
fn spellings(case: &Case, kinds: &[&str]) -> Vec<ExternalIdentity> {
    let mut all = Vec::new();
    for kind in kinds {
        for kind in [
            ExternalObjectKind::parse(kind),
            ExternalObjectKind::Other((*kind).into()),
        ] {
            for instance in case.instances {
                for namespace in case.namespaces {
                    for id in case.ids {
                        all.push(ExternalIdentity {
                            provider: case.provider.clone(),
                            instance: (*instance).into(),
                            namespace: namespace.map(Into::into),
                            kind: kind.clone(),
                            external_id: (*id).into(),
                        });
                    }
                }
            }
        }
    }
    all
}

fn one_key(spellings: &[ExternalIdentity]) -> ObjectKey {
    let key = spellings[0].object_key();
    for spelling in spellings {
        let canonical = spelling.canonical();
        canonical
            .validate(ExternalReferenceId::new())
            .unwrap_or_else(|error| panic!("{spelling:?}: {error}"));
        assert_eq!(canonical.canonical(), canonical, "idempotent {spelling:?}");
        assert_eq!(spelling.object_key(), key, "{spelling:?} splits {key:?}");
    }
    key
}

#[test]
fn every_spelling_of_one_object_shares_one_key_and_spaces_never_merge() {
    let mut keys: Vec<(ExternalProvider, ObjectKey)> = Vec::new();
    let mut checked = 0;
    for case in cases() {
        let mut provider_keys = Vec::new();
        for kinds in case.spaces {
            let spellings = spellings(&case, kinds);
            checked += spellings.len();
            provider_keys.push(one_key(&spellings));
        }
        for (i, key) in provider_keys.iter().enumerate() {
            for other in &provider_keys[i + 1..] {
                assert_ne!(key, other, "{:?} merges two number spaces", case.provider);
            }
        }
        keys.extend(
            provider_keys
                .into_iter()
                .map(|k| (case.provider.clone(), k)),
        );
    }
    assert!(
        checked > 20_000,
        "the generator covers the product: {checked}"
    );
    for (i, (provider, key)) in keys.iter().enumerate() {
        for (other_provider, other) in &keys[i + 1..] {
            let same_family = matches!(
                (provider, other_provider),
                (ExternalProvider::Forgejo, ExternalProvider::Gitea)
            );
            assert_eq!(key == other, same_family, "{key:?} vs {other:?}");
        }
    }
}

#[test]
fn another_instance_port_namespace_or_number_is_another_object() {
    for case in cases() {
        let base = &spellings(&case, case.spaces[0])[0];
        let key = base.object_key();
        let port = ExternalIdentity {
            instance: "git.example:3000".into(),
            ..base.clone()
        };
        let number = ExternalIdentity {
            external_id: base.external_id.replace('6', "7"),
            ..base.clone()
        };
        assert_ne!(port.object_key(), key, "{:?} port", case.provider);
        assert_ne!(number.object_key(), key, "{:?} number", case.provider);
        if base.namespace.is_some() {
            let namespace = ExternalIdentity {
                namespace: Some("o/other".into()),
                ..base.clone()
            };
            assert_ne!(namespace.object_key(), key, "{:?} namespace", case.provider);
        }
    }
}

#[test]
fn a_second_record_for_any_spelling_of_a_recorded_object_is_rejected() {
    let base = fixture();
    let (a, b) = (work(&base, "TEST-A"), work(&base, "TEST-B"));
    for case in cases() {
        let mut plan = base.clone();
        for kinds in case.spaces {
            let first = spellings(&case, kinds)[0].canonical();
            insert(&mut plan, reference(first, a, ExternalLinkRole::Tracks));
        }
        plan.validate()
            .unwrap_or_else(|error| panic!("{:?}: one record per space: {error}", case.provider));
        for kinds in case.spaces {
            for spelling in spellings(&case, kinds).iter().step_by(7) {
                let mut second = plan.clone();
                insert(
                    &mut second,
                    reference(spelling.canonical(), b, ExternalLinkRole::Tracks),
                );
                let error = second.validate().expect_err("second record");
                assert!(
                    error.to_string().contains("already recorded"),
                    "{spelling:?}: {error}"
                );
            }
        }
    }
}

#[test]
fn kinds_outside_a_closed_table_are_rejected() {
    let identity = |provider, kind: &str| ExternalIdentity {
        provider,
        instance: "git.example".into(),
        namespace: Some("o/r".into()),
        kind: ExternalObjectKind::parse(kind),
        external_id: "6".into(),
    };
    for (provider, kind) in [
        (ExternalProvider::GitHub, "ticket"),
        (ExternalProvider::GitHub, "pull_request_review"),
        (ExternalProvider::Forgejo, "discussion"),
        (ExternalProvider::Gitea, "epic"),
        (ExternalProvider::GitLab, "requirement"),
        (ExternalProvider::GitLab, "snippet"),
        (ExternalProvider::GitLab, "discussion"),
    ] {
        let error = identity(provider.clone(), kind)
            .canonical()
            .validate(ExternalReferenceId::new())
            .expect_err(kind);
        assert!(error.to_string().contains("number space"), "{error}");
    }
    let jira = ExternalIdentity {
        namespace: None,
        external_id: "PROJ-6".into(),
        ..identity(ExternalProvider::Jira, "pull_request")
    };
    assert!(
        jira.canonical()
            .validate(ExternalReferenceId::new())
            .is_err()
    );
}

#[test]
fn the_kind_normalizer_folds_case_separators_and_one_plural() {
    for (name, folded) in [
        ("Work Items", "workitem"),
        ("work-item", "workitem"),
        ("STORIES", "story"),
        ("Sub-Tasks", "subtask"),
        ("PRs", "pr"),
        ("class", "class"),
        ("s", "s"),
    ] {
        assert_eq!(fold_kind(name), folded, "{name}");
    }
}
