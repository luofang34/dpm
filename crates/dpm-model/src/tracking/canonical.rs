//! Spelling normalization and the single collision key used by validation, the engine and
//! adapter lookup.

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
    /// The recorded kind stays authoritative for kind-specific rules such as `Merged`; only
    /// the collision check ignores it where issues and pull requests share numbers.
    #[must_use]
    pub fn object_key(&self) -> Self {
        let mut key = self.clone();
        if self.provider.shares_issue_and_review_numbers()
            && key.kind == ExternalObjectKind::PullRequest
        {
            key.kind = ExternalObjectKind::Issue;
        }
        key
    }
}

/// Repeat a stripping step until it removes nothing; each step returns a shorter subslice or
/// the input, so the loop terminates.
fn strip_to_fixed_point(text: &str, step: impl Fn(&str) -> &str) -> &str {
    let mut current = text;
    loop {
        let next = step(current);
        if next.len() == current.len() {
            return current;
        }
        current = next;
    }
}

fn canonical_instance(instance: &str) -> String {
    let lower = instance.to_lowercase();
    strip_to_fixed_point(&lower, |text| {
        let text = text.trim();
        let text = text
            .strip_prefix("https://")
            .or_else(|| text.strip_prefix("http://"))
            .unwrap_or(text)
            .trim_end_matches('/');
        // Default HTTP(S) ports address the same host, so they must not split identity.
        text.strip_suffix(":443")
            .or_else(|| text.strip_suffix(":80"))
            .unwrap_or(text)
    })
    .to_owned()
}

fn canonical_namespace(provider: &ExternalProvider, namespace: &str) -> String {
    let folded = if provider.folds_namespace_case() {
        namespace.to_lowercase()
    } else {
        namespace.to_owned()
    };
    let strips_suffix = provider.numbers_repository_objects();
    strip_to_fixed_point(&folded, |text| {
        let text = text.trim().trim_matches('/');
        match text.strip_suffix(".git") {
            Some(repository) if strips_suffix => repository,
            _ => text,
        }
    })
    .to_owned()
}

fn canonical_external_id(provider: &ExternalProvider, external_id: &str) -> String {
    let external_id = strip_to_fixed_point(external_id, |text| {
        let text = text.trim();
        text.strip_prefix(['#', '!']).unwrap_or(text)
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
        external_id.to_owned()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
