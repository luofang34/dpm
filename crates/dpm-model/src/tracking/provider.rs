//! The closed per-provider table. Canonical spelling, validation, object keys, the kind-change
//! rule and URL host matching all read these rows and nothing else, so a rule decided for one
//! provider or kind applies to every path that handles it.
//!
//! A kind resolves to a number space: two identities name one object exactly when family,
//! instance, namespace, number space and normalized id are equal.

use super::{ExternalObjectKind, ExternalProvider};

/// Whether identities of a provider carry a namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NamespaceRule {
    /// Numbers or keys repeat across repositories or workspaces.
    Required,
    /// Keys already name their project and are unique per instance.
    Forbidden,
    /// Unknown providers decide for themselves.
    Optional,
}

/// Spelling of a provider's object identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IdForm {
    /// A positive decimal number; leading zeros and `#`, `!`, `&` prefixes are spelling.
    Number,
    /// `PROJECT-N`: project case and leading zeros of `N` are spelling.
    Key,
    /// An opaque identifier of an unknown provider.
    Raw,
}

/// How kinds outside a provider's listed rows are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OtherKinds {
    /// Rejected: an unlisted kind could give a number a second identity.
    Rejected,
    /// Accepted as labels of one number space.
    Label(&'static str),
    /// Each kind is its own number space (unknown providers).
    OwnSpace,
}

/// One accepted kind: its canonical name, extra folded synonyms and its number space.
pub(super) struct KindRow {
    pub(super) name: &'static str,
    pub(super) synonyms: &'static [&'static str],
    pub(super) space: &'static str,
}

/// One provider's rules.
pub(super) struct ProviderRules {
    /// Family used in object keys; Forgejo keeps Gitea's paths and numbering.
    pub(super) family: fn() -> ExternalProvider,
    pub(super) namespace: NamespaceRule,
    pub(super) folds_namespace_case: bool,
    pub(super) strips_git_suffix: bool,
    pub(super) ids: IdForm,
    pub(super) kinds: &'static [KindRow],
    pub(super) other_kinds: OtherKinds,
    /// Whether the provider hosts pull or merge requests at all.
    pub(super) reviews: bool,
    /// A public host that is also served as `www.<host>`.
    pub(super) www_host: Option<&'static str>,
}

const ISSUE: &str = "issue";
const PULL_REQUEST: &str = "pull_request";
const REVIEW_SYNONYMS: &[&str] = &["pull", "pr", "mergerequest", "mr"];

const fn row(
    name: &'static str,
    synonyms: &'static [&'static str],
    space: &'static str,
) -> KindRow {
    KindRow {
        name,
        synonyms,
        space,
    }
}

const GITHUB_KINDS: &[KindRow] = &[
    row(ISSUE, &[], "issue"),
    row(PULL_REQUEST, REVIEW_SYNONYMS, "issue"),
    row("discussion", &[], "issue"),
];
const FORGEJO_KINDS: &[KindRow] = &[
    row(ISSUE, &[], "issue"),
    row(PULL_REQUEST, REVIEW_SYNONYMS, "issue"),
];
/// GitLab work item types numbered in the project issue IID sequence, merge requests in their
/// own project sequence, and epics in the group's sequence.
const GITLAB_KINDS: &[KindRow] = &[
    row(ISSUE, &[], "issue"),
    row("incident", &[], "issue"),
    row("task", &[], "issue"),
    row("test_case", &[], "issue"),
    row("ticket", &[], "issue"),
    row("objective", &[], "issue"),
    row("key_result", &[], "issue"),
    row("work_item", &[], "issue"),
    row(PULL_REQUEST, REVIEW_SYNONYMS, "merge_request"),
    row("epic", &[], "epic"),
];
const KEYED_KINDS: &[KindRow] = &[row(ISSUE, &[], "key")];
const OTHER_KINDS: &[KindRow] = &[
    row(ISSUE, &[], ISSUE),
    row(PULL_REQUEST, REVIEW_SYNONYMS, PULL_REQUEST),
];

const fn forge(
    family: fn() -> ExternalProvider,
    kinds: &'static [KindRow],
    www_host: Option<&'static str>,
) -> ProviderRules {
    ProviderRules {
        family,
        namespace: NamespaceRule::Required,
        folds_namespace_case: true,
        strips_git_suffix: true,
        ids: IdForm::Number,
        kinds,
        other_kinds: OtherKinds::Rejected,
        reviews: true,
        www_host,
    }
}

const GITHUB: ProviderRules = forge(
    || ExternalProvider::GitHub,
    GITHUB_KINDS,
    Some("github.com"),
);
const GITLAB: ProviderRules = forge(|| ExternalProvider::GitLab, GITLAB_KINDS, None);
const FORGEJO: ProviderRules = forge(|| ExternalProvider::Forgejo, FORGEJO_KINDS, None);
const JIRA: ProviderRules = ProviderRules {
    family: || ExternalProvider::Jira,
    namespace: NamespaceRule::Forbidden,
    folds_namespace_case: false,
    strips_git_suffix: false,
    ids: IdForm::Key,
    kinds: KEYED_KINDS,
    other_kinds: OtherKinds::Label("key"),
    reviews: false,
    www_host: None,
};
const LINEAR: ProviderRules = ProviderRules {
    family: || ExternalProvider::Linear,
    namespace: NamespaceRule::Required,
    folds_namespace_case: true,
    ..JIRA
};
const OTHER: ProviderRules = ProviderRules {
    family: || ExternalProvider::Other(String::new()),
    namespace: NamespaceRule::Optional,
    folds_namespace_case: false,
    strips_git_suffix: false,
    ids: IdForm::Raw,
    kinds: OTHER_KINDS,
    other_kinds: OtherKinds::OwnSpace,
    reviews: true,
    www_host: None,
};

impl ExternalProvider {
    /// This provider's row of the table.
    pub(super) fn rules(&self) -> &'static ProviderRules {
        match self {
            Self::GitHub => &GITHUB,
            Self::GitLab => &GITLAB,
            Self::Forgejo | Self::Gitea => &FORGEJO,
            Self::Jira => &JIRA,
            Self::Linear => &LINEAR,
            Self::Other(_) => &OTHER,
        }
    }

    /// Family compared in object keys.
    pub(super) fn family(&self) -> ExternalProvider {
        match self {
            Self::Other(_) => self.clone(),
            known => (known.rules().family)(),
        }
    }
}

/// Lowercase letters and digits of a kind name, without one plural ending (`ies` reads as `y`,
/// a single trailing `s` is dropped, `ss` is kept).
pub(super) fn fold_kind(name: &str) -> String {
    let folded: String = name
        .chars()
        // Lower-casing first lets the filter drop marks it produces (`İ` -> `i` + U+0307), so a
        // second fold yields the same name.
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect();
    if let Some(stem) = folded.strip_suffix("ies").filter(|stem| !stem.is_empty()) {
        format!("{stem}y")
    } else if folded.len() > 1 && folded.ends_with('s') && !folded.ends_with("ss") {
        folded[..folded.len() - 1].to_owned()
    } else {
        folded
    }
}

/// A kind folded without a provider: issue and review synonyms become their variants.
pub(super) fn neutral_kind(name: &str) -> ExternalObjectKind {
    OTHER.canonical_kind(&ExternalObjectKind::Other(name.to_owned()))
}

fn kind_name(kind: &ExternalObjectKind) -> &str {
    match kind {
        ExternalObjectKind::Issue => ISSUE,
        ExternalObjectKind::PullRequest => PULL_REQUEST,
        ExternalObjectKind::Other(name) => name,
    }
}

pub(super) fn kind_named(name: &str) -> ExternalObjectKind {
    match name {
        ISSUE => ExternalObjectKind::Issue,
        PULL_REQUEST => ExternalObjectKind::PullRequest,
        other => ExternalObjectKind::Other(other.to_owned()),
    }
}

impl ProviderRules {
    fn row(&self, kind: &ExternalObjectKind) -> Option<&'static KindRow> {
        let folded = fold_kind(kind_name(kind));
        self.kinds
            .iter()
            .find(|row| fold_kind(row.name) == folded || row.synonyms.contains(&folded.as_str()))
    }

    /// Canonical kind: a listed row's name, the neutral issue or review kind, or the folded name.
    pub(super) fn canonical_kind(&self, kind: &ExternalObjectKind) -> ExternalObjectKind {
        match self.row(kind).or_else(|| OTHER.row(kind)) {
            Some(row) => kind_named(row.name),
            None => kind_named(&fold_kind(kind_name(kind))),
        }
    }

    /// The number space of a canonical kind, or `None` when the provider does not accept it.
    pub(super) fn space(&self, kind: &ExternalObjectKind) -> Option<String> {
        if *kind == ExternalObjectKind::PullRequest && !self.reviews {
            return None;
        }
        match (self.row(kind), self.other_kinds) {
            (Some(row), _) => Some(row.space.to_owned()),
            (None, OtherKinds::Label(space)) => Some(space.to_owned()),
            (None, OtherKinds::OwnSpace) => Some(kind_name(kind).to_owned()),
            (None, OtherKinds::Rejected) => None,
        }
    }

    /// Kind names this provider accepts, for rejection messages.
    pub(super) fn accepted_kinds(&self) -> String {
        let names: Vec<_> = self.kinds.iter().map(|row| row.name).collect();
        match self.other_kinds {
            OtherKinds::Rejected => names.join(", "),
            _ => format!("{} or any other kind name", names.join(", ")),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
