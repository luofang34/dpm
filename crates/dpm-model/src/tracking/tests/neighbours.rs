//! Each rule is checked on the case it names and on its nearest neighbour: kind aliases and
//! synonyms, key spellings, real provider links, handles that look like userinfo, and secret
//! names that only differ from a listed one by a suffix or a compound.

use super::{fixture, insert, reference, work};
use crate::{
    ExternalIdentity, ExternalLinkRole, ExternalObjectKind, ExternalProvider, ExternalReferenceId,
};

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

fn assert_one_object(spellings: &[ExternalIdentity]) {
    for spelling in spellings {
        spelling
            .validate(ExternalReferenceId::new())
            .unwrap_or_else(|error| panic!("{spelling:?}: {error}"));
        assert!(
            spelling.object_key() == spellings[0].object_key(),
            "{spelling} and {} name one object",
            spellings[0]
        );
    }
}

#[test]
fn gitlab_kind_aliases_and_issue_types_share_the_issue_number_space() {
    let gitlab = |kind: &str| object(ExternalProvider::GitLab, Some("ops/dpm"), kind, "6");
    assert_one_object(&[
        gitlab("epic"),
        gitlab("epics"),
        gitlab("Epics"),
        gitlab("EPIC"),
    ]);
    assert_one_object(&[
        gitlab("issue"),
        gitlab("incident"),
        gitlab("incidents"),
        gitlab("work_item"),
        gitlab("Work Items"),
        gitlab("task"),
        gitlab("test_case"),
    ]);
    assert_one_object(&[
        gitlab("merge_request"),
        gitlab("MRs"),
        gitlab("pull_request"),
    ]);
    assert!(gitlab("issue").object_key() != gitlab("merge_request").object_key());
    assert!(gitlab("issue").object_key() != gitlab("epic").object_key());
    assert!(gitlab("epic").object_key() != gitlab("merge_request").object_key());
    for unknown in ["widget", "snippet", "requirement"] {
        assert!(
            gitlab(unknown)
                .validate(ExternalReferenceId::new())
                .is_err(),
            "GitLab kind {unknown} has no known number space"
        );
    }
}

#[test]
fn jira_and_linear_kinds_label_one_key() {
    let jira = |kind: &str, id: &str| object(ExternalProvider::Jira, None, kind, id);
    assert_one_object(&[
        jira("issue", "PROJ-6"),
        jira("bug", "PROJ-6"),
        jira("Story", "PROJ-6"),
        jira("Sub-Task", "proj-6"),
        jira("epic", "PROJ-06"),
        jira("issue", "PROJ-006"),
    ]);
    assert_eq!(jira("issue", "PROJ-06").external_id, "PROJ-6");
    assert!(jira("issue", "PROJ-6").object_key() != jira("issue", "PROJ-60").object_key());
    let linear = |kind: &str, id: &str| object(ExternalProvider::Linear, Some("acme"), kind, id);
    assert_one_object(&[
        linear("issue", "ENG-6"),
        linear("task", "eng-6"),
        linear("Sub-issue", "ENG-06"),
    ]);
}

#[test]
fn forge_ids_are_numbers_so_one_object_has_one_spelling() {
    let github = |id: &str| object(ExternalProvider::GitHub, Some("o/r"), "issue", id);
    assert_one_object(&[github("6"), github("#06"), github("006")]);
    for id in ["I_kwDOabc", "0", "6a"] {
        assert!(
            github(id).validate(ExternalReferenceId::new()).is_err(),
            "{id}"
        );
    }
    let www = ExternalIdentity {
        instance: "www.github.com".into(),
        ..github("6")
    }
    .canonical();
    let bare = ExternalIdentity {
        instance: "github.com".into(),
        ..github("6")
    }
    .canonical();
    assert!(
        www.object_key() == bare.object_key(),
        "www.github.com is github.com"
    );
}

fn check_reference(url: Option<&str>, label: &str) -> Result<(), String> {
    let mut plan = fixture();
    let a = work(&plan, "TEST-A");
    let identity = |provider, instance: &str, namespace: Option<&str>, id: &str| ExternalIdentity {
        provider,
        instance: instance.into(),
        namespace: namespace.map(Into::into),
        kind: ExternalObjectKind::Issue,
        external_id: id.into(),
    };
    let host = url
        .and_then(|url| url.split('/').nth(2))
        .unwrap_or("github.com")
        .trim_start_matches("www.");
    let target = match host {
        "gitlab.com" => identity(ExternalProvider::GitLab, host, Some("o/r"), "6"),
        "codeberg.org" => identity(ExternalProvider::Forgejo, host, Some("o/r"), "6"),
        "acme.atlassian.net" => identity(ExternalProvider::Jira, host, None, "PROJ-6"),
        _ => identity(ExternalProvider::GitHub, host, Some("o/r"), "6"),
    };
    let mut value = reference(target, a, ExternalLinkRole::Tracks);
    value.url = url.map(Into::into);
    value.label = label.into();
    insert(&mut plan, value);
    plan.validate().map_err(|error| error.to_string())
}

#[test]
fn real_provider_links_are_accepted() {
    for url in [
        "https://github.com/o/r/issues/6?notification_referrer_id=NT_kwDOAB&utm_source=email",
        "https://github.com/o/r/pull/6/checks?check_run_id=123456",
        "https://www.github.com/o/r/issues/6",
        "https://gitlab.com/o/r/-/merge_requests/6/diffs?commit_id=0a1b2c",
        "https://codeberg.org/o/r/pulls/6/files?style=split&whitespace=",
        "https://acme.atlassian.net/jira/software/projects/PROJ/boards/1?selectedIssue=PROJ-6",
        "https://acme.atlassian.net/browse/PROJ-6?focusedCommentId=10&page=com.atlassian.jira.plugin.system.issuetabpanels:comment-tabpanel#comment-10",
        "https://acme.atlassian.net/issues/?jql=reporter%3Dalice%40corp.example",
    ] {
        assert_eq!(check_reference(Some(url), "Tracked"), Ok(()), "{url}");
    }
}

#[test]
fn fediverse_handles_are_labels_but_userinfo_with_a_path_is_not() {
    for label in ["Ask @alice@mastodon.social", "cc @bob@hachyderm.io."] {
        assert_eq!(check_reference(None, label), Ok(()), "{label}");
    }
    for label in ["see tok@github.com/o/r", "see @tok@github.com/o/r"] {
        assert!(check_reference(None, label).is_err(), "{label}");
    }
    let nested = "https://github.com/o/r/issues/6?next=https://tok@evil.example/x";
    assert!(
        check_reference(Some(nested), "Tracked").is_err(),
        "{nested}"
    );
}

#[test]
fn secret_names_with_suffixes_and_compounds_are_rejected() {
    for name in [
        "token2",
        "password1",
        "tokenValue",
        "token_value",
        "sessionid",
        "PHPSESSID",
        "JSESSIONID",
        "session_id",
        "sid",
        "bearer",
        "code",
        "auth_code",
    ] {
        let url = format!("https://github.com/o/r/issues/6?{name}=abc");
        let label = format!("see {url}");
        assert!(check_reference(None, &label).is_err(), "label {label}");
        let error = check_reference(Some(&url), "Tracked").expect_err(&url);
        assert!(error.contains("secret"), "{url}: {error}");
    }
    for name in [
        "max_tokens",
        "secret_santa",
        "language_code",
        "user_id",
        "tab",
    ] {
        let url = format!("https://github.com/o/r/issues/6?{name}=abc");
        assert_eq!(check_reference(Some(&url), "Tracked"), Ok(()), "{url}");
    }
}
