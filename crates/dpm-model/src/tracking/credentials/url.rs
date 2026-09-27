//! Structural allowlist for the display URL, applied after the shared credential detector.
//!
//! A URL is `http://` or `https://` followed directly by an authority equal to the identity's
//! instance (after the same canonicalization, so `:0443` is the default port), without userinfo or
//! encoding. Its decoded path holds only letters, digits and `-._~/`; its query is `&`-separated
//! `name=value` pairs named in [`QUERY_NAMES`]; its fragment is a plain anchor. Everything else,
//! including `;` parameters and `=` in a path or fragment, is rejected.

use super::{check_text, decode};
use crate::tracking::canonical::canonical_instance;

/// Query parameters that select a view of an issue or review page and never carry a secret.
const QUERY_NAMES: &[&str] = &[
    "tab",
    "page",
    "view",
    "plain",
    "diff",
    "w",
    "focusedcommentid",
];

/// Validate a display URL for the identity on `instance`.
pub(in crate::tracking) fn check_url(url: &str, instance: &str) -> Result<(), &'static str> {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("URL must not contain whitespace");
    }
    check_text(url)?;
    if url.contains('\\') {
        return Err("URL must use forward slashes");
    }
    let rest = strip_scheme(url).ok_or("URL must use http or https")?;
    if rest.starts_with('/') {
        return Err("URL must have exactly two slashes after the scheme");
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.contains(['@', '%']) {
        return Err("shared plans cannot carry URL credentials");
    }
    if canonical_instance(authority) != instance {
        return Err("URL host must match the identity instance");
    }
    let (location, fragment) = tail.split_once('#').unwrap_or((tail, ""));
    let (path, query) = location.split_once('?').unwrap_or((location, ""));
    if !decode(path)?
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | '_' | '~' | '/'))
    {
        return Err("URL path may contain only letters, digits and - . _ ~ /");
    }
    if !query.is_empty() && !query.split('&').all(allowed_query_pair) {
        return Err("URL query may carry only tab, page, view, plain, diff, w or focusedCommentId");
    }
    if !fragment.chars().all(plain) {
        return Err("URL fragment must be a plain anchor");
    }
    Ok(())
}

fn strip_scheme(url: &str) -> Option<&str> {
    ["https://", "http://"].iter().find_map(|scheme| {
        url.get(..scheme.len())
            .filter(|prefix| prefix.eq_ignore_ascii_case(scheme))
            .and_then(|_| url.get(scheme.len()..))
    })
}

fn allowed_query_pair(pair: &str) -> bool {
    pair.split_once('=').is_some_and(|(name, value)| {
        QUERY_NAMES.contains(&name.to_ascii_lowercase().as_str()) && value.chars().all(plain)
    })
}

fn plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~')
}
