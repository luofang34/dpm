//! The public events this adapter understands, and the allowlist that produces them.
//!
//! A provider line is read once, field by field, into [`PublicEvent`]s that hold only what the
//! adapter retains: identities, tool names, bounded and redacted one-line details, and outcomes.
//! Reasoning and thinking blocks, signatures, account, plugin and command listings, usage, costs,
//! tool arguments beyond one identifying value, and tool output are never copied out; an event or
//! block that is not named here is counted and dropped. Nothing here performs I/O.

use crate::redact::{identifier, public_text};
use serde_json::Value;
use thiserror::Error;

mod blocks;
mod control;
mod tools;

pub use tools::tool_detail;

/// Longest one-line detail retained for a tool, a request or an error.
pub const DETAIL_BYTES: usize = 160;

/// Longest public assistant text retained as one progress record.
pub const TEXT_BYTES: usize = 240;

/// What the adapter retains from a provider's stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicEvent {
    /// The session began; configuration facts the provider itself announced.
    Initialized(Session),
    /// The provider answered one of this adapter's control requests.
    Answered {
        /// The request that was answered.
        request: String,
        /// Whether the provider reports success.
        ok: bool,
    },
    /// Public assistant text.
    Text {
        /// Identity of the provider event that carried it.
        id: String,
        /// Bounded, redacted text.
        text: String,
    },
    /// A tool call began.
    ToolUse {
        /// The provider's tool-use identity.
        id: String,
        /// The tool.
        tool: String,
        /// One identifying value of its arguments, bounded and redacted.
        detail: String,
    },
    /// A tool call produced a result.
    ToolResult {
        /// The tool-use identity it answers.
        id: String,
        /// Whether the provider marks it an error.
        error: bool,
        /// `ok` with a size, or a bounded error excerpt; never the output of a successful call.
        detail: String,
    },
    /// The provider asks for permission or input before it proceeds.
    InputRequest {
        /// The provider's control-request identity, which a reply must carry.
        request: String,
        /// The tool-use identity the request belongs to, when it has one.
        tool_use: Option<String>,
        /// The tool.
        tool: String,
        /// One identifying value, bounded and redacted.
        detail: String,
    },
    /// A control request this adapter does not support.
    UnsupportedControl {
        /// Its identity.
        request: String,
        /// Its subtype.
        subtype: String,
    },
    /// The provider is retrying after an error.
    Retry {
        /// Identity of the retry notice.
        key: String,
        /// What it said, bounded.
        detail: String,
    },
    /// The terminal result recapped a denied request.
    Denied {
        /// The tool-use identity.
        tool_use: Option<String>,
        /// The tool.
        tool: String,
        /// One identifying value.
        detail: String,
    },
    /// The provider's explicit terminal result.
    Finished(Finish),
    /// A result event that does not carry the typed evidence a terminal result needs. It ends
    /// nothing.
    MalformedResult {
        /// What was missing.
        reason: String,
    },
}

/// Configuration facts the provider announces when a session starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The provider's session identity.
    pub id: String,
    /// The model it runs.
    pub model: Option<String>,
    /// Its permission mode, as it reports it.
    pub permission_mode: Option<String>,
    /// The runtime's version.
    pub version: Option<String>,
    /// Where it says its credential comes from, a label and never a credential.
    pub key_source: Option<String>,
    /// How many tools it advertises.
    pub tool_count: usize,
}

/// What a well-formed terminal result says became of the turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The turn finished: subtype `success`, no error flagged, and the provider's own reason is
    /// `completed`.
    Completed,
    /// The turn ended in an error the provider reports.
    Failed,
    /// The provider reports that the turn was interrupted or cancelled.
    Interrupted,
}

/// A provider's terminal result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finish {
    /// What the provider says became of the turn, from its subtype, its boolean error flag and its
    /// stated reason together; no one of them alone promotes a turn to completed.
    pub verdict: Verdict,
    /// The result subtype, such as `success` or `error_max_turns`.
    pub subtype: String,
    /// The provider's stated reason the turn ended.
    pub terminal_reason: Option<String>,
    /// How many turns it took.
    pub turns: Option<u64>,
    /// A bounded error excerpt, only when the provider flags an error.
    pub error: Option<String>,
}

/// What a line held that was left out, counted and never kept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dropped {
    /// Reasoning blocks: thinking and redacted thinking.
    pub hidden_reasoning: u32,
    /// Content blocks of a kind the allowlist does not name.
    pub unlisted_blocks: u32,
    /// Events of a kind the allowlist does not name.
    pub unlisted_events: u32,
    /// Events or blocks that lack the provider identity the adapter needs to record them once;
    /// they are refused, never given an invented identity.
    pub unidentified: u32,
}

/// What one line produced.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    /// The public events, in the order the line carried them.
    pub events: Vec<PublicEvent>,
    /// What was left out.
    pub dropped: Dropped,
    /// The provider session the line says it belongs to; control messages carry none.
    pub session: Option<String>,
    /// Whether the line belongs to a subagent's turn rather than the root conversation.
    pub subagent: bool,
}

/// A line that is not a provider event.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    /// The line is not JSON.
    #[error("not JSON: {0}")]
    NotJson(String),
    /// The line is JSON but not an event object.
    #[error("not an event object")]
    NotAnEvent,
}

/// A string field of an object.
fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// A short label field, bounded and redacted.
fn label(value: &Value, key: &str) -> Option<String> {
    field(value, key).map(|text| public_text(text, 80))
}

/// Read one line of the provider's output into the public events it holds.
pub fn parse_line(line: &str) -> Result<Parsed, ParseError> {
    let value: Value = serde_json::from_str(line)
        .map_err(|error| ParseError::NotJson(public_text(&error.to_string(), 80)))?;
    if !value.is_object() {
        return Err(ParseError::NotAnEvent);
    }
    let mut parsed = Parsed {
        session: field(&value, "session_id").and_then(identifier),
        subagent: value
            .get("parent_tool_use_id")
            .is_some_and(|parent| !parent.is_null()),
        ..Parsed::default()
    };
    match (field(&value, "type"), field(&value, "subtype")) {
        (Some("system"), Some("init")) => session(&value, &mut parsed),
        (Some("system"), Some("api_retry")) => retry(&value, &mut parsed),
        (Some("assistant"), _) => blocks::assistant(&value, &mut parsed),
        (Some("user"), _) => blocks::user(&value, &mut parsed),
        (Some("result"), _) => blocks::result(&value, &mut parsed),
        (Some("control_request"), _) => control::request(&value, &mut parsed),
        (Some("control_response"), _) => parsed.events.extend(control::response(&value)),
        _ => count(&mut parsed.dropped.unlisted_events),
    }
    Ok(parsed)
}

fn count(counter: &mut u32) {
    *counter = counter.wrapping_add(1);
}

fn session(value: &Value, parsed: &mut Parsed) {
    let Some(id) = parsed.session.clone() else {
        count(&mut parsed.dropped.unidentified);
        return;
    };
    parsed.events.push(PublicEvent::Initialized(Session {
        id,
        model: label(value, "model"),
        permission_mode: label(value, "permissionMode"),
        version: label(value, "claude_code_version"),
        key_source: label(value, "apiKeySource"),
        tool_count: value
            .get("tools")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
    }));
}

/// A retry notice, identified by the provider's own event identity. One without it is refused.
fn retry(value: &Value, parsed: &mut Parsed) {
    let number = |key: &str| value.get(key).and_then(Value::as_u64);
    let Some(key) = field(value, "uuid").and_then(identifier) else {
        count(&mut parsed.dropped.unidentified);
        return;
    };
    let detail = format!(
        "attempt {} of {}, status {}",
        number("attempt").map_or_else(|| "?".into(), |n| n.to_string()),
        number("max_retries").map_or_else(|| "?".into(), |n| n.to_string()),
        value.get("error_status").map_or_else(
            || "unknown".into(),
            |status| public_text(&status.to_string(), 40)
        ),
    );
    parsed.events.push(PublicEvent::Retry { key, detail });
}

#[cfg(test)]
mod tests;
