//! Rejection of credentials in shared tracking data; plans are exported, diffed and committed.
//!
//! One detector decides for labels and URLs. A URL is first checked exactly as a label word, then
//! against structural rules, so the URL field can be stricter than a label but never looser.
//!
//! Each word is percent-decoded until stable, and `\` is read as `/`, before either rule runs:
//!
//! - **Userinfo.** After an optional `scheme:` and any number of slashes, the authority runs to the
//!   next `/`, `?` or `#`; any `@` in it is userinfo. Anywhere else in the word (a query value, a
//!   nested URL, a word without a scheme), an `@` preceded by a name and followed by a host
//!   (dotted name, `localhost`, `[IPv6]`, or `name:port`) is userinfo when a path, a port or a
//!   `:` in the name makes it more than an address. Userinfo is rejected unless it is exactly
//!   `git`, the public SSH user of every forge (`git@github.com:o/r.git`). A bare address
//!   (`user@example.com`, also after `mailto:`, in a query value, or as a fediverse handle
//!   `@alice@mastodon.social`) is not userinfo.
//! - **Secret parameters.** The word is split at `?`, `&`, `;`, `/` and `#`. A piece `name=value`
//!   with a nonempty value is a secret when [`is_secret_name`] flags the name.

mod url;
pub(super) use url::check_url;

/// Last words that name a secret only as a whole word; `monkey` or `bypass` stay ordinary.
const SECRET_WORDS: &[&str] = &[
    "key", "apikey", "sig", "pwd", "pass", "jwt", "auth", "sid", "bearer",
];
/// Endings that name a secret, so joined spellings such as `accesstoken` are caught too.
const SECRET_SUFFIXES: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "signature",
    "session",
    "sessionid",
    "sessid",
    "credential",
    "credentials",
    "authorization",
];
/// Trailing words that qualify the secret before them (`tokenValue`, `session_id`).
const QUALIFIERS: &[&str] = &[
    "value", "val", "str", "string", "id", "data", "raw", "hex", "b64", "base64", "text",
];
/// Words before `code` that make it an OAuth or device authorization code.
const CODE_GRANTS: &[&str] = &[
    "auth",
    "oauth",
    "access",
    "authorization",
    "device",
    "verification",
];
/// Characters that end the name before an `@` inside a word.
const NAME_BREAKS: &[char] = &['/', '?', '&', '#', '=', ';', '+', ','];
/// Decoding rounds; more nested encodings than this are rejected rather than inspected.
const DECODE_ROUNDS: usize = 4;

/// Reject credentials in free text, one whitespace-separated word at a time.
pub(super) fn check_text(text: &str) -> Result<(), &'static str> {
    for word in text.split_whitespace() {
        let word = word
            .trim_matches(|c: char| matches!(c, '(' | ')' | '<' | '>' | '"' | '\''))
            .trim_end_matches(['.', ',', ';', ':', '!', '?']);
        check_word(word)?;
    }
    Ok(())
}

fn check_word(word: &str) -> Result<(), &'static str> {
    let decoded = decode(word)?.replace('\\', "/");
    if carries_userinfo(&decoded.to_lowercase()) {
        return Err("shared plans cannot carry URL credentials");
    }
    for piece in decoded.split(['?', '&', ';', '/', '#']) {
        if let Some((name, value)) = piece.split_once('=')
            && !value.is_empty()
            && is_secret_name(name)
        {
            return Err("shared plans cannot carry secret parameters");
        }
    }
    Ok(())
}

fn carries_userinfo(word: &str) -> bool {
    if let Some(address) = word.strip_prefix("mailto:") {
        return !is_email(address);
    }
    if is_email(word) || word.strip_prefix('@').is_some_and(is_email) {
        return false;
    }
    let (has_scheme, rest) = match scheme_split(word) {
        Some(rest) => (true, rest),
        None => (false, word),
    };
    let after_slashes = rest.trim_start_matches('/');
    if !has_scheme && after_slashes.len() == rest.len() {
        return embedded_userinfo(word);
    }
    let end = after_slashes
        .find(['/', '?', '#'])
        .unwrap_or(after_slashes.len());
    let (authority, remainder) = after_slashes.split_at(end);
    authority
        .rsplit_once('@')
        .is_some_and(|(userinfo, _)| userinfo != "git")
        || embedded_userinfo(remainder)
}

/// An `@` after a name and before a host, unless the pair is a bare address: no path, query or
/// fragment right after the host, no port, and no `:` in the name.
fn embedded_userinfo(text: &str) -> bool {
    text.match_indices('@').any(|(at, _)| {
        let (before, after) = text.split_at(at);
        // Inside an authority (after `//`, as in `scheme://` or a protocol-relative URL) anywhere
        // in the word, an `@` always ends userinfo, whatever precedes it (`x=https://tok@host`,
        // `see:https://tok@host`, `?next=https://tok@host`).
        let in_authority = before
            .rfind("//")
            .and_then(|start| before.get(start + 2..))
            .is_some_and(|authority| {
                !authority.is_empty() && !authority.contains(['/', '?', '#', '\\'])
            });
        if in_authority {
            return true;
        }
        let name = before.rsplit(NAME_BREAKS).next().unwrap_or(before);
        let after = after.get(1..).unwrap_or_default();
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        let host = after.get(..end).unwrap_or(after);
        let host = host.split(NAME_BREAKS).next().unwrap_or(host);
        let address = !name.contains(':')
            && !host.contains(':')
            && host.contains('.')
            && !after
                .get(host.len()..)
                .unwrap_or_default()
                .starts_with(['/', '?', '#']);
        !name.is_empty() && name != "git" && is_host(host) && !address
    })
}

/// The text after a `scheme:` prefix; a colon followed by a digit is a port, not a scheme.
fn scheme_split(word: &str) -> Option<&str> {
    let (scheme, rest) = word.split_once(':')?;
    let mut chars = scheme.chars();
    let scheme_like = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
    let port = rest.starts_with(|c: char| c.is_ascii_digit());
    (scheme_like && !port).then_some(rest)
}

fn is_email(word: &str) -> bool {
    let Some((local, domain)) = word.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !local.contains([':', '/', '?', '#'])
        && !domain.contains([':', '/', '?', '#', '@', '['])
        && domain.contains('.')
        && is_host(domain)
}

/// A host token: `[IPv6]`, `localhost`, a dotted name or `name:port`, optionally followed by a
/// port or an SCP-style `:path`.
fn is_host(token: &str) -> bool {
    if token.starts_with('[') {
        return true;
    }
    let (host, port) = token.split_once(':').unwrap_or((token, ""));
    let dotted = host.contains('.') && host.split('.').all(|label| !label.is_empty());
    let has_port = !port.is_empty() && port.chars().all(|c| c.is_ascii_digit());
    let named = host == "localhost" || dotted || has_port;
    !host.is_empty()
        && named
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Whether a parameter name names a secret.
///
/// The name is split into words at `_`, `-`, `.`, `:` and lower-to-upper case changes; trailing
/// digits of each word are dropped (`token2`), and trailing [`QUALIFIERS`] are dropped while a
/// word remains before them (`tokenValue`, `session_id`). The last remaining word is a secret when
/// it is one of [`SECRET_WORDS`] or ends with one of [`SECRET_SUFFIXES`] (`PHPSESSID`), or when it
/// is `code` alone or after one of [`CODE_GRANTS`] (`auth_code`). Whole words decide, so
/// `max_tokens`, `secret_santa` and `language_code` are ordinary.
fn is_secret_name(name: &str) -> bool {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for c in name.chars() {
        if matches!(c, '_' | '-' | '.' | ':') || (c.is_uppercase() && previous_lower) {
            words.push(std::mem::take(&mut current));
        }
        if !matches!(c, '_' | '-' | '.' | ':') {
            current.extend(c.to_lowercase());
        }
        previous_lower = c.is_lowercase() || c.is_ascii_digit();
    }
    words.push(current);
    let mut words: Vec<String> = words
        .iter()
        .map(|word| {
            word.trim_end_matches(|c: char| c.is_ascii_digit())
                .to_owned()
        })
        .filter(|word| !word.is_empty())
        .collect();
    while words.len() > 1
        && words
            .last()
            .is_some_and(|last| QUALIFIERS.contains(&last.as_str()))
    {
        words.pop();
    }
    let Some(last) = words.last() else {
        return false;
    };
    if last == "code" {
        return words.len() == 1
            || words
                .iter()
                .rev()
                .nth(1)
                .is_some_and(|grant| CODE_GRANTS.contains(&grant.as_str()));
    }
    SECRET_WORDS.contains(&last.as_str())
        || SECRET_SUFFIXES.iter().any(|suffix| last.ends_with(suffix))
}

/// Percent-decode until nothing changes, so double encoding cannot hide a name or an `@`.
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
