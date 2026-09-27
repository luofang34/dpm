//! Generated link suite: providers × number spaces × kind aliases × identifier spellings ×
//! instance and port spellings × namespace spellings. Each number space gets its own record and
//! owner, and every other spelling of that object is refused a second tracking owner.

use super::rules::linked;
use super::*;

struct Case {
    provider: ExternalProvider,
    namespaces: &'static [Option<&'static str>],
    ids: &'static [&'static str],
    spaces: &'static [&'static [&'static str]],
}

const INSTANCES: &[&str] = &[
    "git.example",
    "Git.Example:0443",
    "https://git.example:443/",
    "git.example.",
];
const REPOSITORIES: &[Option<&str>] = &[Some("o/r"), Some("./O//R.git/"), Some("O/r")];
const NUMBERS: &[&str] = &["6", "#006", " 06 "];
const OWNERS: &[&str] = &["TEST-A", "TEST-B", "TEST-C", "TEST-D"];

const GITLAB_SPACES: &[&[&str]] = &[
    &[
        "Issue",
        "issues",
        "incident",
        "Incidents",
        "task",
        "Test Cases",
        "ticket",
        "objective",
        "Key Results",
        "work_item",
        "Work Items",
    ],
    &["PullRequest", "merge_requests", "MRs", "Merge Request"],
    &["epic", "epics", "EPICS"],
];

fn cases() -> Vec<Case> {
    let shared: &'static [&'static [&'static str]] = &[&[
        "Issue",
        "issues",
        "PullRequest",
        "pull-requests",
        "Pull Requests",
        "prs",
        "MR",
    ]];
    vec![
        Case {
            provider: ExternalProvider::GitHub,
            namespaces: REPOSITORIES,
            ids: NUMBERS,
            spaces: &[&[
                "Issue",
                "issues",
                "PullRequest",
                "pulls",
                "prs",
                "discussion",
                "Discussions",
            ]],
        },
        Case {
            provider: ExternalProvider::Forgejo,
            namespaces: REPOSITORIES,
            ids: NUMBERS,
            spaces: shared,
        },
        Case {
            provider: ExternalProvider::Gitea,
            namespaces: REPOSITORIES,
            ids: NUMBERS,
            spaces: shared,
        },
        Case {
            provider: ExternalProvider::GitLab,
            namespaces: REPOSITORIES,
            ids: &["6", "#006", "!6", "&06"],
            spaces: GITLAB_SPACES,
        },
        Case {
            provider: ExternalProvider::Jira,
            namespaces: &[None],
            ids: &["PROJ-6", "proj-06", "#Proj-006"],
            spaces: &[&[
                "Issue", "bug", "Bugs", "Story", "stories", "Sub-task", "epic",
            ]],
        },
        Case {
            provider: ExternalProvider::Linear,
            namespaces: &[Some("acme"), Some("/Acme/")],
            ids: &["ENG-6", "eng-06", "#ENG-006"],
            spaces: &[&["Issue", "issues", "task", "Sub-issue"]],
        },
        Case {
            provider: ExternalProvider::Other("custom".into()),
            namespaces: &[Some("o/r"), Some("./o//r/")],
            ids: &["6", "#6", "!6"],
            spaces: &[
                &["Issue", "issues"],
                &["PullRequest", "pulls"],
                &["widget", "Widgets"],
                &["gadget", "Gadgets"],
            ],
        },
    ]
}

fn spellings(case: &Case, kinds: &[&str]) -> Vec<ExternalIdentity> {
    let mut all = Vec::new();
    for kind in kinds {
        for kind in [
            ExternalObjectKind::parse(kind),
            ExternalObjectKind::Other((*kind).into()),
        ] {
            for instance in INSTANCES {
                for namespace in case.namespaces {
                    for external_id in case.ids {
                        all.push(
                            ExternalIdentity {
                                provider: case.provider.clone(),
                                instance: (*instance).into(),
                                namespace: namespace.map(Into::into),
                                kind: kind.clone(),
                                external_id: (*external_id).into(),
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

#[test]
fn every_object_has_exactly_one_tracking_owner_and_spaces_never_merge() {
    let mut checked = 0;
    for case in cases() {
        let mut plan = fixture();
        let tracks = ExternalLinkRole::Tracks;
        for (kinds, owner) in case.spaces.iter().zip(OWNERS) {
            let work = id(&plan, owner);
            linked(
                &mut plan,
                work,
                spellings(&case, kinds)[0].clone(),
                tracks,
                None,
            )
            .unwrap_or_else(|error| panic!("{:?} {owner}: {error}", case.provider));
        }
        assert_eq!(
            plan.external_references.len(),
            case.spaces.len(),
            "{:?}: one record per number space",
            case.provider
        );
        let other = id(&plan, "TEST-E");
        for (kinds, owner) in case.spaces.iter().zip(OWNERS) {
            for spelling in spellings(&case, kinds) {
                checked += 1;
                let error = linked(&mut plan, other, spelling.clone(), tracks, None)
                    .expect_err("a second tracking owner");
                let EngineError::TrackingOwned { owner: actual, .. } = &error else {
                    panic!("{spelling}: {error}");
                };
                assert_eq!(actual.to_string(), *owner, "{spelling} owned elsewhere");
            }
        }
        assert_eq!(plan.external_references.len(), case.spaces.len());
    }
    assert!(
        checked > 2_000,
        "the generator covers the product: {checked}"
    );
}
