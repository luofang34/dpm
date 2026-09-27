use crate::{ExternalIdentity, ExternalObjectKind, ExternalProvider, ExternalReferenceId};

fn providers() -> Vec<ExternalProvider> {
    let mut providers = vec![
        ExternalProvider::GitHub,
        ExternalProvider::GitLab,
        ExternalProvider::Forgejo,
        ExternalProvider::Gitea,
        ExternalProvider::Jira,
        ExternalProvider::Linear,
    ];
    for name in ["GitHub", " Forgejo ", "Custom", "custom"] {
        providers.push(ExternalProvider::Other(name.into()));
    }
    providers
}

fn instances() -> Vec<String> {
    let mut instances = Vec::new();
    for scheme in ["", "https://", "HTTP://", "https://http://", " "] {
        for host in ["Git.Example", "codeberg.org"] {
            for tail in [
                "", ":443", ":80", ":3000", "/", ":443/", "/:443", " / ", ":80:443",
            ] {
                instances.push(format!("{scheme}{host}{tail}"));
            }
        }
    }
    instances
}

const NAMESPACES: &[Option<&str>] = &[
    None,
    Some("O/R"),
    Some("/o/r/"),
    Some("o/r.git"),
    Some("o/r.GIT"),
    Some("O/R.Git.git"),
    Some("o/r.git/"),
    Some("o/r/.git"),
    Some(" o/r .git "),
    Some(".git"),
    Some("Ops/Dpm.Git/ "),
];

const IDS: &[&str] = &[
    "42", "#42", "!042", "##7", "# 7", " #0 ", "000", "proj-1", "#proj-1", "Eng-1", "#!0Ab",
];

fn kinds() -> Vec<ExternalObjectKind> {
    vec![
        ExternalObjectKind::Issue,
        ExternalObjectKind::PullRequest,
        ExternalObjectKind::Other("Pulls".into()),
        ExternalObjectKind::Other(" MR ".into()),
        ExternalObjectKind::Other("Weird".into()),
    ]
}

/// Every generated spelling, crossing each component's awkward forms with every provider.
fn spellings() -> Vec<ExternalIdentity> {
    let (providers, instances, kinds) = (providers(), instances(), kinds());
    let mut all = Vec::new();
    for provider in &providers {
        for instance in &instances {
            for (n, namespace) in NAMESPACES.iter().enumerate() {
                for (i, external_id) in IDS.iter().enumerate() {
                    all.push(ExternalIdentity {
                        provider: provider.clone(),
                        instance: instance.clone(),
                        namespace: namespace.map(str::to_owned),
                        kind: kinds[(n + i) % kinds.len()].clone(),
                        external_id: (*external_id).into(),
                    });
                }
            }
        }
    }
    all
}

#[test]
fn canonical_is_idempotent_over_generated_spellings() {
    let spellings = spellings();
    assert!(spellings.len() > 10_000, "the generator covers the product");
    for spelling in &spellings {
        let once = spelling.canonical();
        assert_eq!(
            once.canonical(),
            once,
            "canonical is not idempotent for {spelling:?}"
        );
        assert_eq!(once.object_key().canonical(), once.object_key());
        // The spelling a rejection suggests must itself be accepted as canonical.
        if let Err(error) = once.validate(ExternalReferenceId::new()) {
            assert!(
                !error.to_string().contains("not canonical"),
                "{spelling:?} -> {once:?}: {error}"
            );
        }
    }
}

#[test]
fn repository_suffix_and_prefix_spellings_reach_one_form() {
    let forge = |namespace: &str, id: &str| {
        ExternalIdentity {
            provider: ExternalProvider::Gitea,
            instance: "codeberg.org".into(),
            namespace: Some(namespace.into()),
            kind: ExternalObjectKind::Issue,
            external_id: id.into(),
        }
        .canonical()
    };
    let canonical = forge("ops/dpm", "6");
    for namespace in [
        "ops/dpm.GIT",
        "Ops/Dpm.git.git",
        "ops/dpm.git/",
        " /ops/dpm .git/ ",
    ] {
        assert_eq!(forge(namespace, "6"), canonical, "{namespace}");
    }
    for id in ["##6", "# 6", "!#006"] {
        assert_eq!(forge("ops/dpm", id), canonical, "{id}");
    }
    let instance = ExternalIdentity {
        instance: "HTTPS://http://Codeberg.org/:443/".into(),
        ..canonical.clone()
    };
    assert_eq!(instance.canonical(), canonical);
}
