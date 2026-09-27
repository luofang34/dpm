use crate::{ActorId, ExternalReferenceId, ValidationError, WorkItemId, validation::invalid};
use chrono::{DateTime, Utc};
use provider::{IdForm, NamespaceRule};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod canonical;
mod credentials;
mod provider;
mod validation;
pub use canonical::ObjectKey;
pub(crate) use validation::validate;

/// Tracker or forge family; its row in the provider table decides namespaces, kinds, number
/// spaces and identifier spelling.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ExternalProvider {
    /// GitHub, hosted or Enterprise Server.
    GitHub,
    /// GitLab, hosted or self-managed.
    GitLab,
    /// A Forgejo instance.
    Forgejo,
    /// A Gitea instance.
    Gitea,
    /// Jira, Cloud or Data Center.
    Jira,
    /// Linear.
    Linear,
    /// Another family, named in lowercase.
    Other(String),
}

impl ExternalProvider {
    /// Case-insensitive family name; unknown names become a lowercase `Other` family.
    pub fn parse(name: &str) -> Self {
        let name = name.trim().to_lowercase();
        match name.as_str() {
            "github" => Self::GitHub,
            "gitlab" => Self::GitLab,
            "forgejo" => Self::Forgejo,
            "gitea" => Self::Gitea,
            "jira" => Self::Jira,
            "linear" => Self::Linear,
            _ => Self::Other(name),
        }
    }

    fn display_name(&self) -> &str {
        match self {
            Self::Other(name) => name,
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::Forgejo => "Forgejo",
            Self::Gitea => "Gitea",
            Self::Jira => "Jira",
            Self::Linear => "Linear",
        }
    }
}

/// Category of external object. The provider table maps each kind to a number space, so a kind
/// separates identity only where the provider numbers it separately.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ExternalObjectKind {
    /// An issue or ticket.
    Issue,
    /// A pull request or merge request.
    PullRequest,
    /// Another provider object category, named in lowercase.
    Other(String),
}

impl ExternalObjectKind {
    /// Provider-neutral kind, folded by the one kind normalizer: case, separators and one plural
    /// ending are ignored, so `pull_requests`, `PRs` and `merge-request` are `PullRequest`,
    /// `issues` is `Issue` and `Work Items` is `{"Other":"workitem"}`. Canonicalization then maps
    /// the name onto the provider's own kind table.
    pub fn parse(name: &str) -> Self {
        provider::neutral_kind(name)
    }

    fn name(&self) -> &str {
        match self {
            Self::Issue => "issue",
            Self::PullRequest => "pull_request",
            Self::Other(name) => name,
        }
    }
}

/// Identity of an external object, independent of its display label and URL.
///
/// Two references name one object when their [`ObjectKey`]s are equal, so equal numbers from
/// different instances, namespaces or number spaces never collide.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalIdentity {
    /// Tracker family.
    pub provider: ExternalProvider,
    /// Lowercase `host[:port]` of the hosted or self-hosted instance, without scheme or path.
    pub instance: String,
    /// Tenant, owner/repository or project namespace where the provider scopes identifiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Object category; it splits identity only between number spaces.
    pub kind: ExternalObjectKind,
    /// Provider identifier: a number on forges, a `PROJECT-N` key on Jira and Linear.
    pub external_id: String,
}

impl ExternalIdentity {
    /// Check that this identity is canonical and structurally valid; commands check a requested
    /// identity with this even when it resolves to an existing record.
    pub fn validate(&self, id: ExternalReferenceId) -> Result<(), ValidationError> {
        if self.instance.contains('@') {
            return Err(invalid(
                "external identity",
                id,
                "shared plans cannot carry credentials in the instance",
            ));
        }
        // Structure is checked on the canonical form so a suggested spelling is always valid.
        let canonical = self.canonical();
        canonical.validate_parts(id)?;
        if canonical != *self {
            return Err(invalid(
                "external identity",
                id,
                format!("identity is not canonical; use {canonical}"),
            ));
        }
        Ok(())
    }

    fn validate_parts(&self, id: ExternalReferenceId) -> Result<(), ValidationError> {
        let fail = |reason: &str| Err(invalid("external identity", id, reason));
        if !valid_instance(&self.instance) {
            return fail(
                "instance must be a lowercase host[:port] without scheme or path, port 1-65535",
            );
        }
        if let ExternalProvider::Other(name) = &self.provider
            && !lowercase_word(name)
        {
            return fail("provider name must be a lowercase word");
        }
        if let ExternalObjectKind::Other(name) = &self.kind
            && !lowercase_word(name)
        {
            return fail("object kind name must be a lowercase word");
        }
        let rules = self.provider.rules();
        match (self.namespace.as_deref(), rules.namespace) {
            (None, NamespaceRule::Required) => {
                return fail("this provider scopes identifiers by namespace; supply it");
            }
            (Some(_), NamespaceRule::Forbidden) => {
                return fail("this provider's keys are unique per instance; omit the namespace");
            }
            (Some(namespace), _) if !valid_namespace(namespace) => {
                return fail("namespace must be slash-separated names without `..` or credentials");
            }
            _ => {}
        }
        if self.kind == ExternalObjectKind::PullRequest && !rules.reviews {
            return fail("this provider has no pull requests");
        }
        if rules.space(&self.kind).is_none() {
            return Err(invalid(
                "external identity",
                id,
                format!(
                    "{} has no {} kind with a known number space; use {}",
                    self.provider.display_name(),
                    self.kind.name(),
                    rules.accepted_kinds()
                ),
            ));
        }
        if !valid_external_id(rules.ids, &self.external_id) {
            return fail(match rules.ids {
                IdForm::Number => "this provider numbers objects; use a positive number such as 42",
                IdForm::Key => "this provider keys objects as PROJECT-N, such as PROJ-6",
                IdForm::Raw => "external id must be a nonempty provider identifier",
            });
        }
        Ok(())
    }
}

impl std::fmt::Display for ExternalIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.provider.display_name(), self.instance)?;
        if let Some(namespace) = &self.namespace {
            write!(f, "/{namespace}")?;
        }
        write!(f, ":{}:{}", self.kind.name(), self.external_id)
    }
}

/// How linked work relates to an external object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ExternalLinkRole {
    /// This work is the single local owner tracking the external object.
    Tracks,
    /// The external object is related context for this work.
    Relates,
}

/// One work item's link to an external object; it is context, never evidence or a gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalLink {
    /// Linked work, referenced by stable identity so key renames keep the link.
    pub work: WorkItemId,
    /// Relationship of the work to the external object.
    pub role: ExternalLinkRole,
}

/// External lifecycle state as last reported by an actor; the local lifecycle stays authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExternalState {
    /// The external object is open.
    Open,
    /// The external object is closed.
    Closed,
    /// The pull request is merged.
    Merged,
}

/// An attributed report of external state; it never changes local work status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalObservation {
    /// Reported state.
    pub state: ExternalState,
    /// Time at which the state was recorded.
    pub observed_at: DateTime<Utc>,
    /// Principal that reported the state.
    pub observed_by: ActorId,
}

impl ExternalObservation {
    /// Whether a later report of `next` may replace this one. A merged pull request can neither
    /// reopen nor close, so after `Merged` only another `Merged` report is accepted from any link.
    #[must_use]
    pub fn admits(&self, next: ExternalState) -> bool {
        self.state != ExternalState::Merged || next == ExternalState::Merged
    }
}

/// External object known to the workspace, with its display data and local links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalReference {
    /// Stable local identity; it survives relabeling and namespace changes.
    pub id: ExternalReferenceId,
    /// Provider identity, unique within the workspace.
    pub identity: ExternalIdentity,
    /// Human-readable display label; not part of identity.
    pub label: String,
    /// Optional browser address on the identity's instance; not part of identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Most recent reported external state, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<ExternalObservation>,
    /// Linked work; at most one link per work item and one `Tracks` link overall.
    pub links: BTreeSet<ExternalLink>,
}

impl ExternalReference {
    /// The work item that tracks this external object, if any.
    #[must_use]
    pub fn tracking_owner(&self) -> Option<WorkItemId> {
        self.links
            .iter()
            .find(|link| link.role == ExternalLinkRole::Tracks)
            .map(|link| link.work)
    }
}

fn valid_external_id(form: IdForm, id: &str) -> bool {
    let positive = |digits: &str| {
        digits.starts_with(|c: char| ('1'..='9').contains(&c))
            && digits.chars().all(|c| c.is_ascii_digit())
    };
    match form {
        IdForm::Number => positive(id),
        IdForm::Key => id.rsplit_once('-').is_some_and(|(project, number)| {
            project.starts_with(|c: char| c.is_ascii_uppercase())
                && project
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                && positive(number)
        }),
        IdForm::Raw => {
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-'))
        }
    }
}

fn valid_instance(instance: &str) -> bool {
    let (host, port) = instance.split_once(':').unwrap_or((instance, ""));
    let port_in_range = port.is_empty()
        || (!port.starts_with('0') && port.parse::<u16>().is_ok_and(|port| port > 0));
    let host_ok = !host.is_empty()
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && host
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-'));
    let port_ok = !instance.contains(':')
        || (!port.is_empty() && port.len() <= 5 && port.chars().all(|c| c.is_ascii_digit()));
    host_ok && port_ok && port_in_range
}

fn valid_namespace(namespace: &str) -> bool {
    namespace.split('/').all(|segment| {
        !matches!(segment, "" | "." | "..")
            && segment
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '~' | '-'))
    })
}

fn lowercase_word(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
