//! Spelling normalization, the single collision key used by validation, the engine and adapter
//! lookup, and the single kind-change rule used by linking and reviewed plan changes.

use super::{ExternalIdentity, ExternalObjectKind, ExternalProvider};

impl ExternalIdentity {
    /// Normalize spelling that providers treat as equal; validation rejects any other form.
    ///
    /// Every rule strips to a fixed point, so `canonical` is idempotent and the spelling a
    /// rejection suggests is always accepted.
    #[must_use]
    pub fn canonical(&self) -> Self {
        let provider = match &self.provider {
            ExternalProvider::Other(name) => ExternalProvider::parse(name),
            known => known.clone(),
        };
        let namespace = self
            .namespace
            .as_deref()
            .map(|namespace| canonical_namespace(&provider, namespace))
            .filter(|namespace| !namespace.is_empty());
        let kind = match &self.kind {
            ExternalObjectKind::Other(name) => ExternalObjectKind::parse(name),
            known => known.clone(),
        };
        Self {
            instance: canonical_instance(&self.instance),
            namespace,
            kind,
            external_id: canonical_external_id(&provider, &self.external_id),
            provider,
        }
    }

    /// The part of a canonical identity that decides whether two records name one object.
    ///
    /// Forgejo is a fork of Gitea that keeps its repository paths and issue/pull numbering, so
    /// one instance addressed as either family names the same objects; the key uses Forgejo for
    /// both. Other families stay distinct even on one host, and every family stays distinct
    /// across instances. On GitHub, Forgejo and Gitea every kind of the shared issue sequence
    /// (issues, pull requests and GitHub discussions) keys as `Issue`.
    ///
    /// The recorded provider and kind stay authoritative for display and for kind-specific rules
    /// such as `Merged`; only the collision check folds them.
    #[must_use]
    pub fn object_key(&self) -> Self {
        let mut key = self.clone();
        if key.provider == ExternalProvider::Gitea {
            key.provider = ExternalProvider::Forgejo;
        }
        if self.provider.numbers_in_issue_sequence(&self.kind) {
            key.kind = ExternalObjectKind::Issue;
        }
        key
    }

    /// Whether a record of this identity may take `next`'s kind: unchanged, or refined from issue
    /// to pull request where both kinds share one number. A pull request is the more specific
    /// kind of that object, so a record is never downgraded; every other kind change names a
    /// different object. Linking and reviewed plan changes both apply this rule.
    #[must_use]
    pub fn permits_kind_change_to(&self, next: &Self) -> bool {
        self.kind == next.kind
            || (self.kind == ExternalObjectKind::Issue
                && next.kind == ExternalObjectKind::PullRequest
                && self.provider.shares_issue_and_review_numbers()
                && next.provider.shares_issue_and_review_numbers())
    }

    /// Whether this identity names `recorded`'s object with a kind the record may be refined to.
    #[must_use]
    pub fn refines_kind_of(&self, recorded: &Self) -> bool {
        self.kind != recorded.kind
            && recorded.permits_kind_change_to(self)
            && self.object_key() == recorded.object_key()
    }
}

/// Repeat a normalization step until it changes nothing; every step only removes text, so the
/// loop terminates.
fn to_fixed_point(text: String, step: impl Fn(&str) -> String) -> String {
    let mut current = text;
    loop {
        let next = step(&current);
        if next == current {
            return current;
        }
        current = next;
    }
}

/// Lowercase `host[:port]` without scheme, trailing slash or trailing root dot; a port loses
/// leading zeros, and the default HTTP(S) ports `443` and `80` are dropped.
pub(super) fn canonical_instance(instance: &str) -> String {
    to_fixed_point(instance.to_lowercase(), |text| {
        let text = text.trim();
        let text = text
            .strip_prefix("https://")
            .or_else(|| text.strip_prefix("http://"))
            .unwrap_or(text)
            .trim_end_matches('/');
        let (host, port) = match text.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => (host, Some(port)),
            _ => (text, None),
        };
        let host = host.strip_suffix('.').unwrap_or(host).trim_end_matches('/');
        match port.map(|port| (port, port.trim_start_matches('0'))) {
            None | Some(("", _)) | Some((_, "443" | "80")) => host.to_owned(),
            Some((_, "")) => format!("{host}:0"),
            Some((_, port)) => format!("{host}:{port}"),
        }
    })
}

/// Slash-separated segments without empty or `.` segments; `..` is kept so validation rejects it.
fn canonical_namespace(provider: &ExternalProvider, namespace: &str) -> String {
    let folded = if provider.folds_namespace_case() {
        namespace.to_lowercase()
    } else {
        namespace.to_owned()
    };
    let strips_suffix = provider.numbers_repository_objects();
    to_fixed_point(folded, |text| {
        let joined = text
            .trim()
            .split('/')
            .filter(|segment| !matches!(*segment, "" | "."))
            .collect::<Vec<_>>()
            .join("/");
        match joined.strip_suffix(".git") {
            Some(repository) if strips_suffix => repository.to_owned(),
            _ => joined,
        }
    })
}

fn canonical_external_id(provider: &ExternalProvider, external_id: &str) -> String {
    let external_id = to_fixed_point(external_id.to_owned(), |text| {
        let text = text.trim();
        text.strip_prefix(['#', '!']).unwrap_or(text).to_owned()
    });
    if provider.folds_id_to_upper() {
        external_id.to_uppercase()
    } else if provider.numbers_repository_objects()
        && !external_id.is_empty()
        && external_id.chars().all(|c| c.is_ascii_digit())
    {
        let trimmed = external_id.trim_start_matches('0');
        if trimmed.is_empty() { "0" } else { trimmed }.to_owned()
    } else {
        external_id
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
