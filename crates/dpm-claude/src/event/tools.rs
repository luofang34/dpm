//! The one identifying value of a tool call's arguments that the adapter retains.
//!
//! Arguments are arbitrary: a write carries file contents, a shell call a whole script. The
//! adapter keeps a single value that says what the call is about, such as a path, a pattern or a
//! question, bounded and redacted, and for any tool it does not know it keeps nothing.

use super::DETAIL_BYTES;
use crate::redact::public_text;
use serde_json::Value;

/// The first of `keys` that is a string in `input`, as retained text.
fn first(input: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .map(|text| public_text(text, DETAIL_BYTES))
}

/// A URL without its credentials, query or fragment: scheme, host and path only.
fn without_secrets(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = rest.get(..end).unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let path = rest
        .get(end..)
        .unwrap_or_default()
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    if scheme.is_empty() {
        format!("{host}{path}")
    } else {
        format!("{scheme}://{host}{path}")
    }
}

/// What an `AskUserQuestion` call asks: its questions and their option labels.
fn questions(input: &Value) -> Option<String> {
    let asked = input.get("questions")?.as_array()?;
    let parts: Vec<String> = asked
        .iter()
        .take(2)
        .filter_map(|entry| {
            let question = entry.get("question").and_then(Value::as_str)?;
            let options: Vec<&str> = entry
                .get("options")
                .and_then(Value::as_array)
                .map(|labels| {
                    labels
                        .iter()
                        .take(6)
                        .filter_map(|option| {
                            option
                                .as_str()
                                .or_else(|| option.get("label").and_then(Value::as_str))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(if options.is_empty() {
                question.to_string()
            } else {
                format!("{question} [{}]", options.join("/"))
            })
        })
        .collect();
    (!parts.is_empty()).then(|| public_text(&parts.join(" | "), DETAIL_BYTES))
}

/// One bounded, redacted value saying what `tool` was asked to do; empty when the adapter keeps
/// nothing for it.
#[must_use]
pub fn tool_detail(tool: &str, input: &Value) -> String {
    let found = match tool {
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            first(input, &["file_path", "notebook_path"])
        }
        "Glob" | "Grep" => first(input, &["pattern"]),
        "Bash" => first(input, &["command"]),
        "WebSearch" => first(input, &["query"]),
        "WebFetch" => input
            .get("url")
            .and_then(Value::as_str)
            .map(|url| public_text(&without_secrets(url), DETAIL_BYTES)),
        "AskUserQuestion" => questions(input),
        "Task" | "Agent" => first(input, &["description"]),
        _ => None,
    };
    found.unwrap_or_default()
}
