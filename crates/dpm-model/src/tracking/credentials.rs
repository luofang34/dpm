//! Rejection of credentials in shared tracking data; plans are exported, diffed and committed.
//!
//! Parameter names are percent-decoded until stable, lowercased and stripped of `_`, `-` and
//! `.` before matching, so `%74oken`, `%2574oken`, `API_Key` and `x-api.key` are all caught.

/// Name fragments that mark a parameter as secret wherever it appears.
const SECRET_PARAMETERS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "signature",
    "credential",
    "apikey",
];
/// Short names that are secret as URL parameters but ordinary words in prose.
const SECRET_NAMES: &[&str] = &["key", "sig", "code", "auth", "authorization"];
/// Decoding rounds; more nested encodings than this are rejected rather than inspected.
const DECODE_ROUNDS: usize = 4;

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

/// Reject credentials in free text: URLs with or without a scheme that carry userinfo or a
/// secret parameter, and bare `name=value` pairs with a secret name.
///
/// A word without a scheme is read as a URL only when a host-like authority is followed by a
/// path, query or fragment, so an email address such as `user@example.com` stays ordinary text.
pub(super) fn check_text(text: &str) -> Result<(), &'static str> {
    for word in text.split_whitespace() {
        let word = word
            .trim_matches(|c: char| matches!(c, '(' | ')' | '<' | '>' | '[' | ']' | '"' | '\''))
            .trim_end_matches(['.', ',', ';', ':', '!', '?'])
            .to_lowercase();
        if let Some((_, rest)) = word.split_once("://") {
            check_credentials(rest)?;
        } else if addresses_host(&word) {
            check_credentials(&word)?;
        }
        check_secret_pairs(&word)?;
    }
    Ok(())
}

/// Check the lowercase part of a URL after its scheme and return its authority.
fn check_credentials(rest: &str) -> Result<&str, &'static str> {
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    // Any userinfo, including a token-only `TOKEN@host` or an encoded `%40`, is a credential.
    if decode(authority)?.contains('@') {
        return Err("shared plans cannot carry URL credentials");
    }
    let parameters = tail
        .split_once(['?', '#'])
        .map_or("", |(_, parameters)| parameters);
    for pair in parameters.split(['&', '?', '#', ';']) {
        let name = pair.split_once('=').map_or(pair, |(name, _)| name);
        let name = parameter_name(name)?;
        if SECRET_NAMES.contains(&name.as_str()) || is_secret(&name) {
            return Err("shared plans cannot carry secret URL parameters");
        }
    }
    Ok(authority)
}

/// Reject `name=value` pairs with a secret-looking name anywhere in a word.
fn check_secret_pairs(word: &str) -> Result<(), &'static str> {
    for pair in word.split(['?', '&', '#', ';']) {
        if let Some((name, value)) = pair.split_once('=')
            && !value.is_empty()
            && is_secret(&parameter_name(name)?)
        {
            return Err("shared plans cannot carry secret parameters");
        }
    }
    Ok(())
}

/// A scheme-less word names a host when its authority is a dotted host or carries a port and
/// something follows it.
fn addresses_host(word: &str) -> bool {
    let end = word.find(['/', '?', '#']).unwrap_or(word.len());
    let (authority, tail) = word.split_at(end);
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let (host, port) = host_port.split_once(':').unwrap_or((host_port, ""));
    let host_like = !host.is_empty()
        && host.split('.').all(|label| !label.is_empty())
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '%'));
    let has_port = !port.is_empty() && port.chars().all(|c| c.is_ascii_digit());
    !tail.is_empty() && host_like && (host.contains('.') || has_port)
}

fn is_secret(name: &str) -> bool {
    SECRET_PARAMETERS.iter().any(|secret| name.contains(secret))
}

fn parameter_name(name: &str) -> Result<String, &'static str> {
    Ok(decode(name)?
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | '.'))
        .collect())
}

/// Percent-decode until nothing changes, so double encoding cannot hide a name.
fn decode(text: &str) -> Result<String, &'static str> {
    let mut current = text.to_owned();
    for _ in 0..DECODE_ROUNDS {
        let next = decode_once(&current);
        if next == current {
            return Ok(current);
        }
        current = next;
    }
    Err("shared plans cannot carry repeatedly percent-encoded URL parts")
}

fn decode_once(text: &str) -> String {
    let mut decoded = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&first, tail)) = rest.split_first() {
        let escaped = tail
            .get(..2)
            .filter(|_| first == b'%')
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                rest = tail.get(2..).unwrap_or_default();
            }
            None => {
                decoded.push(first);
                rest = tail;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
