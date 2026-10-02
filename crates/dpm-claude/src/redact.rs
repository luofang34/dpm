//! Redaction and bounding of public text at the adapter boundary.
//!
//! Everything the adapter retains passes through here first. Text is reduced to printable words,
//! secrets and credentials are replaced, home directories are named `~`, and the result is cut to
//! a byte bound on a character boundary. This is a boundary filter, not a promise that arbitrary
//! text is safe: the adapter also retains no tool arguments or outputs beyond what its allowlist
//! names.

/// What replaces removed material.
pub const REDACTED: &str = "<redacted>";

const SECRET_NAMES: [&str; 9] = [
    "token",
    "secret",
    "password",
    "passwd",
    "apikey",
    "api_key",
    "api-key",
    "authorization",
    "credential",
];

const SECRET_PREFIXES: [&str; 14] = [
    "sk-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "xoxb-",
    "xoxa-",
    "xoxp-",
    "xoxr-",
    "xoxs-",
    "AKIA",
    "AIza",
];

/// Punctuation that wraps a word without being part of it.
fn wrapper(character: char) -> bool {
    matches!(
        character,
        '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
    )
}

fn names_a_secret(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SECRET_NAMES.iter().any(|word| lower.contains(word))
}

fn long_run(word: &str, minimum: usize, allowed: impl Fn(char) -> bool) -> bool {
    word.chars().count() >= minimum && word.chars().all(allowed)
}

/// Whether a word has the shape of a credential.
fn looks_secret(word: &str) -> bool {
    if SECRET_PREFIXES
        .iter()
        .any(|prefix| word.starts_with(prefix) && word.len() >= 12)
    {
        return true;
    }
    if word.starts_with("eyJ") && word.matches('.').count() >= 2 && word.len() >= 16 {
        return true;
    }
    long_run(word, 32, |c| c.is_ascii_hexdigit())
        || long_run(word, 40, |c| {
            c.is_ascii_alphanumeric() || matches!(c, '+' | '=' | '_' | '-')
        })
}

/// A home directory written as `~`, so a user's name is not retained.
fn home_relative(word: &str) -> Option<String> {
    let rest = word
        .strip_prefix("/Users/")
        .or_else(|| word.strip_prefix("/home/"))?;
    Some(match rest.split_once('/') {
        Some((_, tail)) => format!("~/{tail}"),
        None => "~".to_string(),
    })
}

/// Where the scrubber is in a credential written as `name = value`, `name: value`, `--name value`
/// or `Bearer value`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// Nothing to remove next.
    Nothing,
    /// A secret's name was seen; a separator may follow.
    Separator,
    /// A command-line flag naming a secret was seen: a separator, or the value itself, follows.
    Flag,
    /// The next word is the secret's value.
    Value,
}

/// An authentication scheme whose credential follows it: `Bearer` always, and `Basic` only straight
/// after a secret's name, since it is also an ordinary word.
fn is_scheme(word: &str, pending: Pending) -> bool {
    word.eq_ignore_ascii_case("bearer")
        || (word.eq_ignore_ascii_case("basic") && pending != Pending::Nothing)
}

/// A URL with its userinfo (`user:password@`) and the values of secret-named query parameters
/// replaced, if it has any.
fn clean_url(core: &str) -> Option<String> {
    let (scheme, rest) = core.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = rest.get(..end).unwrap_or_default();
    let tail = rest.get(end..).unwrap_or_default();
    let host = match authority.rsplit_once('@') {
        Some((_, host)) => format!("{REDACTED}@{host}"),
        None => authority.to_string(),
    };
    let tail = match tail.split_once('?') {
        Some((path, query)) => {
            let query: Vec<String> = query
                .split('&')
                .map(|pair| match pair.split_once('=') {
                    Some((name, _)) if names_a_secret(name) => format!("{name}={REDACTED}"),
                    _ => pair.to_string(),
                })
                .collect();
            format!("{path}?{}", query.join("&"))
        }
        None => tail.to_string(),
    };
    let cleaned = format!("{scheme}://{host}{tail}");
    (cleaned != core).then_some(cleaned)
}

/// The name part of `name=value` or `name:value`, and what follows the separator.
fn assignment(core: &str) -> Option<(&str, &str)> {
    let (name, value) = core.split_once(['=', ':'])?;
    names_a_secret(name).then_some((name, value))
}

/// One word, with its wrapping punctuation kept and its core judged.
fn scrub_word(word: &str, pending: &mut Pending) -> String {
    let core = word.trim_matches(wrapper);
    let lead = word.len() - word.trim_start_matches(wrapper).len();
    let trail = word.len() - word.trim_end_matches(wrapper).len();
    let before = word.get(..lead).unwrap_or_default();
    let after = word
        .get(word.len().saturating_sub(trail)..)
        .unwrap_or_default();
    let separator = matches!(core, "=" | ":" | ":=" | "=>");
    let replaced = if is_scheme(core, *pending) {
        // The label stays; whatever follows it is the credential, even after a named secret.
        *pending = Pending::Value;
        None
    } else if *pending == Pending::Value
        || (*pending == Pending::Flag && !separator && !core.starts_with('-'))
    {
        *pending = Pending::Nothing;
        Some(REDACTED.to_string())
    } else if matches!(*pending, Pending::Separator | Pending::Flag) && separator {
        *pending = Pending::Value;
        None
    } else if let Some(cleaned) = clean_url(core) {
        *pending = Pending::Nothing;
        Some(cleaned)
    } else if let Some((name, value)) = assignment(core) {
        // `name=` is followed by its value, and `name=Basic` by the credential that scheme carries.
        *pending = if value.is_empty() || is_scheme(value, Pending::Separator) {
            Pending::Value
        } else {
            Pending::Nothing
        };
        Some(format!("{name}={REDACTED}"))
    } else if names_a_secret(core) {
        *pending = if core.starts_with('-') {
            Pending::Flag
        } else {
            Pending::Separator
        };
        None
    } else if looks_secret(core) {
        *pending = Pending::Nothing;
        Some(REDACTED.to_string())
    } else {
        *pending = Pending::Nothing;
        home_relative(core)
    };
    match replaced {
        Some(text) => format!("{before}{text}{after}"),
        None => word.to_string(),
    }
}

/// `text` with credentials and home directories removed and whitespace reduced to single spaces.
#[must_use]
pub fn redact(text: &str) -> String {
    let mut pending = Pending::Nothing;
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .map(|word| scrub_word(word, &mut pending))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` cut to at most `limit` bytes on a character boundary, ending in `…` when it was cut and
/// there is room for it. The result never exceeds `limit`.
#[must_use]
pub fn bounded(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let ellipsis = '…'.len_utf8();
    let room = if limit >= ellipsis.saturating_mul(2) {
        limit - ellipsis
    } else {
        limit
    };
    let mut kept = String::new();
    for character in text.chars() {
        if kept.len().saturating_add(character.len_utf8()) > room {
            break;
        }
        kept.push(character);
    }
    if room < limit {
        kept.push('…');
    }
    kept
}

/// Redacted, then bounded: the form in which any provider text is retained.
#[must_use]
pub fn public_text(text: &str, limit: usize) -> String {
    bounded(&redact(text), limit)
}

/// An identifier from the provider as it may appear in retained text: short, made only of the
/// characters identifiers use, and not shaped like a credential. Anything else is not an
/// identifier and is not kept, so provider metadata cannot carry a secret into a retained record.
#[must_use]
pub fn identifier(text: &str) -> Option<String> {
    let usable = !text.is_empty()
        && text.len() <= 100
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        && !looks_secret(text)
        && !names_a_secret(text)
        && home_relative(text).is_none();
    usable.then(|| text.to_string())
}

#[cfg(test)]
mod tests;
