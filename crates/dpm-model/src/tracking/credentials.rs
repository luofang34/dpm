//! Rejection of credentials in shared tracking data; plans are exported, diffed and committed.

/// Query or fragment parameter names that carry secrets in common tracker and storage links.
const SECRET_PARAMETERS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "signature",
    "credential",
    "apikey",
    "api_key",
    "api-key",
];
const SECRET_NAMES: &[&str] = &["key", "sig", "code", "auth", "authorization"];

/// Validate a display URL: http(s), on the identity's instance, with no userinfo or secret parameter.
pub(super) fn check_url(url: &str, instance: &str) -> Result<(), &'static str> {
    if url.chars().any(char::is_whitespace) {
        return Err("URL must not contain whitespace");
    }
    let lower = url.to_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .ok_or("URL must use http or https")?;
    let authority = check_credentials(rest)?;
    if authority != instance {
        return Err("URL host must match the identity instance");
    }
    Ok(())
}

/// Reject any URL embedded in free text that carries userinfo or a secret parameter.
pub(super) fn check_text(text: &str) -> Result<(), &'static str> {
    for word in text.split_whitespace() {
        if let Some((_, rest)) = word.split_once("://") {
            check_credentials(&rest.to_lowercase())?;
        }
    }
    Ok(())
}

/// Check the part of a URL after its scheme and return its authority.
fn check_credentials(rest: &str) -> Result<&str, &'static str> {
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    // Any userinfo, including a token-only `TOKEN@host`, is a credential.
    if authority.contains('@') {
        return Err("shared plans cannot carry URL credentials");
    }
    let parameters = tail
        .split_once(['?', '#'])
        .map_or("", |(_, parameters)| parameters);
    for pair in parameters.split(['&', '?', '#', ';']) {
        let name = pair.split_once('=').map_or(pair, |(name, _)| name);
        if SECRET_NAMES.contains(&name) || SECRET_PARAMETERS.iter().any(|s| name.contains(s)) {
            return Err("shared plans cannot carry secret URL parameters");
        }
    }
    Ok(authority)
}
