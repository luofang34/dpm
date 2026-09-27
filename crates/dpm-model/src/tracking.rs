use crate::{ActorId, ExternalReferenceId, ValidationError, WorkItemId, validation::invalid};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod canonical;
mod credentials;
mod validation;
pub(crate) use validation::validate;

/// Tracker or forge family; it decides which identity parts are required.
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

    /// Repository forges number issues per repository and Linear per workspace, so the
    /// namespace is part of identity there; Jira keys are unique per instance.
    fn requires_namespace(&self) -> bool {
        matches!(
            self,
            Self::GitHub | Self::GitLab | Self::Forgejo | Self::Gitea | Self::Linear
        )
    }

    /// A Jira key already names its project (`PROJ-1`) and is unique per instance, so a separate
    /// namespace would let one issue appear under several identities.
    fn forbids_namespace(&self) -> bool {
        matches!(self, Self::Jira)
    }

    /// Jira and Linear resolve issue keys (`PROJ-1`, `ENG-1`) case-insensitively and display them
    /// upper-case.
    fn folds_id_to_upper(&self) -> bool {
        matches!(self, Self::Jira | Self::Linear)
    }

    /// Forges route owner/repository paths and Linear routes workspace slugs case-insensitively,
    /// so case must not split identity.
    fn folds_namespace_case(&self) -> bool {
        matches!(
            self,
            Self::GitHub | Self::GitLab | Self::Forgejo | Self::Gitea | Self::Linear
        )
    }

    /// Repository forges whose objects carry plain numbers, where leading zeros and a `.git`
    /// repository suffix address the same object.
    fn numbers_repository_objects(&self) -> bool {
        matches!(
            self,
            Self::GitHub | Self::GitLab | Self::Forgejo | Self::Gitea
        )
    }

    /// These forges number issues and pull requests in one sequence, so `#5` is one object
    /// whichever kind a caller names; GitLab numbers merge requests separately.
    fn shares_issue_and_review_numbers(&self) -> bool {
        matches!(self, Self::GitHub | Self::Forgejo | Self::Gitea)
    }

    fn hosts_code_review(&self) -> bool {
        !matches!(self, Self::Jira | Self::Linear)
    }

    /// Kinds numbered in one repository's shared issue sequence. GitHub also numbers discussions
    /// there, so a discussion and an issue with one number cannot coexist.
    fn numbers_in_issue_sequence(&self, kind: &ExternalObjectKind) -> bool {
        match kind {
            ExternalObjectKind::Issue | ExternalObjectKind::PullRequest => {
                self.shares_issue_and_review_numbers()
            }
            ExternalObjectKind::Other(name) => *self == Self::GitHub && name == DISCUSSION,
        }
    }

    /// Shared-number forges accept only the kinds of their one sequence, so no unknown kind can
    /// give one number a second identity; other providers accept any kind they host.
    fn accepts_kind(&self, kind: &ExternalObjectKind) -> bool {
        if self.shares_issue_and_review_numbers() {
            self.numbers_in_issue_sequence(kind)
        } else {
            *kind != ExternalObjectKind::PullRequest || self.hosts_code_review()
        }
    }
}

/// The GitHub discussion kind name.
const DISCUSSION: &str = "discussion";

/// Category of external object; GitLab issues and merge requests have separate numbering.
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
    /// Kind name compared ignoring case, separators and one plural `s`: `pull_requests`, `PRs`
    /// and `merge-request` are `PullRequest`, `issues` is `Issue`, `Discussions` is the
    /// `discussion` kind. Any other name becomes a lowercase `Other` kind.
    pub fn parse(name: &str) -> Self {
        let name = name.trim().to_lowercase();
        let folded: String = name.chars().filter(|c| c.is_alphanumeric()).collect();
        match folded.strip_suffix('s').unwrap_or(&folded) {
            "issue" => Self::Issue,
            "pullrequest" | "pull" | "pr" | "mergerequest" | "mr" => Self::PullRequest,
            DISCUSSION => Self::Other(DISCUSSION.into()),
            _ => Self::Other(name),
        }
    }
}

/// Identity of an external object, independent of its display label and URL.
///
/// Two references are the same object only when every part is equal, so equal numbers from
/// different instances, namespaces or object kinds never collide.
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
    /// Object category within the namespace.
    pub kind: ExternalObjectKind,
    /// Stable provider identifier, such as an issue number or node ID.
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
        match self.namespace.as_deref() {
            None if self.provider.requires_namespace() => {
                return fail("this provider scopes identifiers by namespace; supply it");
            }
            Some(_) if self.provider.forbids_namespace() => {
                return fail("this provider's keys are unique per instance; omit the namespace");
            }
            Some(namespace) if !valid_namespace(namespace) => {
                return fail("namespace must be slash-separated names without `..` or credentials");
            }
            _ => {}
        }
        if !self.provider.accepts_kind(&self.kind) {
            return fail(if self.provider.shares_issue_and_review_numbers() {
                "this forge numbers issues and pull requests (and GitHub discussions) in one \
                 sequence; use kind Issue, PullRequest or, on GitHub, discussion"
            } else {
                "this provider has no pull requests"
            });
        }
        if self.external_id.is_empty()
            || !self
                .external_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-'))
        {
            return fail("external id must be a nonempty provider identifier");
        }
        Ok(())
    }
}

impl std::fmt::Display for ExternalIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let provider = match &self.provider {
            ExternalProvider::Other(name) => name.as_str(),
            ExternalProvider::GitHub => "GitHub",
            ExternalProvider::GitLab => "GitLab",
            ExternalProvider::Forgejo => "Forgejo",
            ExternalProvider::Gitea => "Gitea",
            ExternalProvider::Jira => "Jira",
            ExternalProvider::Linear => "Linear",
        };
        let kind = match &self.kind {
            ExternalObjectKind::Issue => "issue",
            ExternalObjectKind::PullRequest => "pull_request",
            ExternalObjectKind::Other(name) => name.as_str(),
        };
        write!(f, "{provider}:{}", self.instance)?;
        if let Some(namespace) = &self.namespace {
            write!(f, "/{namespace}")?;
        }
        write!(f, ":{kind}:{}", self.external_id)
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
