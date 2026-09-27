//! Spelling normalization, the single object key used by validation, the engine, adapter lookup
//! and reviewed plan changes, and the single kind-change rule; all three read the provider table.

use super::provider::IdForm;
use super::{ExternalIdentity, ExternalObjectKind, ExternalProvider};

/// What decides whether two records name one external object.
///
/// It is the provider family (Gitea and Forgejo are one family), instance, namespace, the number
/// space of the kind in the provider table, and the normalized identifier. Kinds sharing a number
/// space (GitHub issues, pull requests and discussions; GitLab issues, incidents and tasks; every
/// Jira or Linear issue type) share one key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectKey {
    /// Provider family.
    pub family: ExternalProvider,
    /// Canonical `host[:port]`.
    pub instance: String,
    /// Canonical namespace, where the provider scopes identifiers.
    pub namespace: Option<String>,
    /// Number space of the kind; `None` when the provider does not accept the kind.
    pub space: Option<String>,
    /// Normalized identifier.
    pub id: String,
}

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
        let rules = provider.rules();
        Self {
            instance: canonical_instance_for(&provider, &self.instance),
            namespace,
            kind: rules.canonical_kind(&self.kind),
            external_id: canonical_external_id(rules.ids, &self.external_id),
            provider,
        }
    }

    /// The key of the object this identity names, computed on its canonical form.
    #[must_use]
    pub fn object_key(&self) -> ObjectKey {
        let canonical = self.canonical();
        ObjectKey {
            family: canonical.provider.family(),
            space: canonical.provider.rules().space(&canonical.kind),
            instance: canonical.instance,
            namespace: canonical.namespace,
            id: canonical.external_id,
        }
    }

    /// Whether a record of this identity may be restated with `next`'s kind: unchanged, or
    /// another kind of the same number space unless the record is a pull request. A pull request
    /// is the most specific kind of its number, so it is never downgraded; a kind of another
    /// number space names another object. Linking and reviewed plan changes both apply this rule.
    #[must_use]
    pub fn permits_kind_change_to(&self, next: &Self) -> bool {
        let (from, to) = (self.canonical(), next.canonical());
        if from.kind == to.kind {
            return true;
        }
        let space = |identity: &Self| identity.provider.rules().space(&identity.kind);
        from.kind != ExternalObjectKind::PullRequest
            && from.provider.family() == to.provider.family()
            && space(&from).is_some()
            && space(&from) == space(&to)
    }

    /// Whether this identity names `recorded`'s object with a kind the record may be restated to.
    #[must_use]
    pub fn refines_kind_of(&self, recorded: &Self) -> bool {
        self.canonical().kind != recorded.canonical().kind
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
fn canonical_instance(instance: &str) -> String {
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

/// The canonical instance, with `www.` removed from a public host the provider also serves bare.
pub(super) fn canonical_instance_for(provider: &ExternalProvider, instance: &str) -> String {
    let instance = canonical_instance(instance);
    match (provider.rules().www_host, instance.strip_prefix("www.")) {
        (Some(host), Some(bare)) if bare == host => bare.to_owned(),
        _ => instance,
    }
}

/// Slash-separated segments without empty or `.` segments; `..` is kept so validation rejects it.
fn canonical_namespace(provider: &ExternalProvider, namespace: &str) -> String {
    let rules = provider.rules();
    let folded = if rules.folds_namespace_case {
        namespace.to_lowercase()
    } else {
        namespace.to_owned()
    };
    to_fixed_point(folded, |text| {
        let joined = text
            .trim()
            .split('/')
            .filter(|segment| !matches!(*segment, "" | "."))
            .collect::<Vec<_>>()
            .join("/");
        match joined.strip_suffix(".git") {
            Some(repository) if rules.strips_git_suffix => repository.to_owned(),
            _ => joined,
        }
    })
}

fn strip_leading_zeros(digits: &str) -> &str {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() && !digits.is_empty() {
        "0"
    } else {
        trimmed
    }
}

fn canonical_external_id(form: IdForm, external_id: &str) -> String {
    let prefixes: &[char] = match form {
        IdForm::Number => &['#', '!', '&'],
        IdForm::Key | IdForm::Raw => &['#', '!'],
    };
    let external_id = to_fixed_point(external_id.to_owned(), |text| {
        let text = text.trim();
        text.strip_prefix(prefixes).unwrap_or(text).to_owned()
    });
    let numeric = |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_digit());
    match form {
        IdForm::Number if numeric(&external_id) => strip_leading_zeros(&external_id).to_owned(),
        IdForm::Key => {
            let upper = external_id.to_uppercase();
            match upper.rsplit_once('-') {
                Some((project, number)) if numeric(number) => {
                    format!("{project}-{}", strip_leading_zeros(number))
                }
                _ => upper,
            }
        }
        IdForm::Number | IdForm::Raw => external_id,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
