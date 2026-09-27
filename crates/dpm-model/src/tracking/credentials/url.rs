//! Structural rules for the display URL, applied after the shared credential detector.
//!
//! A URL is `http://` or `https://` followed directly by an authority equal to the identity's
//! instance (after the same canonicalization, so `:0443` is the default port and `www.github.com`
//! is `github.com`), without userinfo or encoding. Its decoded path holds only letters, digits and
//! `-._~/:+,`; its fragment is a plain anchor. The query is free-form: any parameter is accepted
//! unless the detector flags its name or value, because real provider links carry tracking,
//! view and permalink parameters that no allowlist can enumerate.

use super::{check_text, decode};
use crate::ExternalIdentity;
use crate::tracking::canonical::canonical_instance_for;

/// Validate a display URL for `identity`.
pub(in crate::tracking) fn check_url(
    url: &str,
    identity: &ExternalIdentity,
) -> Result<(), &'static str> {
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
    if canonical_instance_for(&identity.provider, authority) != identity.instance {
        return Err("URL host must match the identity instance");
    }
    let (location, fragment) = tail.split_once('#').unwrap_or((tail, ""));
    let path = location.split_once('?').map_or(location, |(path, _)| path);
    if !decode(path)?
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | '_' | '~' | '/' | ':' | '+' | ','))
    {
        return Err("URL path may contain only letters, digits and - . _ ~ / : + ,");
    }
    if !fragment
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
    {
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
