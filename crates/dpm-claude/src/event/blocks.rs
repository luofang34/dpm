//! Assistant, user and result events: the content blocks the allowlist names.

use super::{
    DETAIL_BYTES, Finish, Parsed, PublicEvent, TEXT_BYTES, Verdict, field, tools::tool_detail,
};
use crate::redact::{identifier, public_text};
use serde_json::Value;

fn count(counter: &mut u32) {
    *counter = counter.wrapping_add(1);
}

/// The content blocks of an event's message, if it has a block list.
fn blocks(value: &Value) -> Option<&Vec<Value>> {
    value.get("message")?.get("content")?.as_array()
}

/// Identity of a text block: its event's own identity, with the block's position when it is not
/// first. Without an event identity there is none; one is never invented.
fn text_identity(value: &Value, index: usize) -> Option<String> {
    let base = field(value, "uuid").and_then(identifier)?;
    Some(if index == 0 {
        base
    } else {
        format!("{base}.{index}")
    })
}

/// Assistant text and tool calls; reasoning is dropped here and goes no further.
pub(super) fn assistant(value: &Value, parsed: &mut Parsed) {
    let Some(content) = blocks(value) else {
        count(&mut parsed.dropped.unlisted_events);
        return;
    };
    for (index, block) in content.iter().enumerate() {
        match field(block, "type") {
            Some("text") => {
                let text = field(block, "text")
                    .map(|raw| public_text(raw, TEXT_BYTES))
                    .unwrap_or_default();
                if !text.is_empty() {
                    match text_identity(value, index) {
                        Some(id) => parsed.events.push(PublicEvent::Text { id, text }),
                        None => count(&mut parsed.dropped.unidentified),
                    }
                }
            }
            Some("tool_use") => match field(block, "id").and_then(identifier) {
                Some(id) => {
                    let tool = field(block, "name")
                        .and_then(identifier)
                        .unwrap_or_else(|| "unnamed-tool".into());
                    let detail = block
                        .get("input")
                        .map(|input| tool_detail(&tool, input))
                        .unwrap_or_default();
                    parsed
                        .events
                        .push(PublicEvent::ToolUse { id, tool, detail });
                }
                None => count(&mut parsed.dropped.unidentified),
            },
            Some("thinking" | "redacted_thinking") => count(&mut parsed.dropped.hidden_reasoning),
            _ => count(&mut parsed.dropped.unlisted_blocks),
        }
    }
}

/// The text a tool result carries and its size in bytes.
fn result_text(block: &Value) -> (String, usize) {
    match block.get("content") {
        Some(Value::String(text)) => (text.clone(), text.len()),
        Some(Value::Array(parts)) => {
            let joined: Vec<&str> = parts
                .iter()
                .filter_map(|part| {
                    (field(part, "type") == Some("text"))
                        .then(|| field(part, "text"))
                        .flatten()
                })
                .collect();
            let text = joined.join(" ");
            let size = text.len();
            (text, size)
        }
        _ => (String::new(), 0),
    }
}

/// Tool results. A successful call's output is not retained, only that it succeeded and its size.
pub(super) fn user(value: &Value, parsed: &mut Parsed) {
    let Some(content) = blocks(value) else {
        count(&mut parsed.dropped.unlisted_events);
        return;
    };
    for block in content {
        let Some(id) = (field(block, "type") == Some("tool_result"))
            .then(|| field(block, "tool_use_id").and_then(identifier))
            .flatten()
        else {
            count(&mut parsed.dropped.unidentified);
            continue;
        };
        let error = block
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (text, size) = result_text(block);
        let detail = if error {
            public_text(&text, DETAIL_BYTES)
        } else {
            format!("ok, {size} bytes")
        };
        parsed
            .events
            .push(PublicEvent::ToolResult { id, error, detail });
    }
}

/// Whether the provider's stated reason says the turn was cut short rather than ended.
fn interrupted(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    ["interrupt", "cancel", "abort"]
        .iter()
        .any(|word| lower.contains(word))
}

/// What a result says became of the turn, or why it says nothing that can be relied on. A turn
/// completes only when the subtype is `success`, no error is flagged and the provider's own reason is
/// `completed`: a reason that is absent, unknown or says otherwise promotes nothing.
fn verdict(subtype: &str, flagged: bool, reason: Option<&str>) -> Result<Verdict, &'static str> {
    if reason.is_some_and(interrupted) {
        return Ok(Verdict::Interrupted);
    }
    if subtype != "success" || flagged {
        return Ok(Verdict::Failed);
    }
    match reason {
        Some("completed") => Ok(Verdict::Completed),
        Some(_) => Err("the result reports success but an unrecognized terminal reason"),
        None => Err("the result reports success without a terminal reason"),
    }
}

/// The terminal result, and the requests it recaps as denied. A result counts only with explicit
/// typed evidence: a string subtype, a boolean error flag, and a reason that agrees with them.
/// Anything less ends nothing.
pub(super) fn result(value: &Value, parsed: &mut Parsed) {
    let subtype = field(value, "subtype").and_then(identifier);
    let flagged = value.get("is_error").and_then(Value::as_bool);
    let reason = field(value, "terminal_reason").and_then(identifier);
    let malformed = |reason: &str| PublicEvent::MalformedResult {
        reason: reason.to_string(),
    };
    let (Some(subtype), Some(flagged)) = (subtype, flagged) else {
        parsed.events.push(malformed(
            "the result lacks a string subtype or a boolean is_error",
        ));
        return;
    };
    let verdict = match verdict(&subtype, flagged, reason.as_deref()) {
        Ok(verdict) => verdict,
        Err(why) => {
            parsed.events.push(malformed(why));
            return;
        }
    };
    let error = (verdict != Verdict::Completed).then(|| {
        field(value, "result")
            .map(|text| public_text(text, DETAIL_BYTES))
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| format!("the provider reported {subtype}"))
    });
    for denial in value
        .get("permission_denials")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let tool = field(denial, "tool_name")
            .and_then(identifier)
            .unwrap_or_else(|| "unnamed-tool".into());
        let detail = denial
            .get("tool_input")
            .map(|input| tool_detail(&tool, input))
            .unwrap_or_default();
        let tool_use = field(denial, "tool_use_id").and_then(identifier);
        parsed.events.push(PublicEvent::Denied {
            tool_use,
            tool,
            detail,
        });
    }
    parsed.events.push(PublicEvent::Finished(Finish {
        verdict,
        subtype,
        terminal_reason: reason,
        turns: value.get("num_turns").and_then(Value::as_u64),
        error,
    }));
}
