//! Control messages of the documented stream-json protocol: permission requests and the answers to
//! this adapter's own requests.

use super::{DETAIL_BYTES, Parsed, PublicEvent, field, tools::tool_detail};
use crate::redact::{identifier, public_text};
use serde_json::Value;

/// A request from the provider. Only `can_use_tool` is supported; any other subtype is reported
/// as unsupported so the adapter can refuse it. A request without an identity cannot be answered,
/// so it is refused as unidentified and the provider's own deadline applies.
pub(super) fn request(value: &Value, parsed: &mut Parsed) {
    let Some(request_id) = field(value, "request_id").and_then(identifier) else {
        parsed.dropped.unidentified = parsed.dropped.unidentified.wrapping_add(1);
        return;
    };
    let body = value.get("request");
    let subtype = body
        .and_then(|body| field(body, "subtype"))
        .and_then(identifier);
    let (Some(body), Some("can_use_tool")) = (body, subtype.as_deref()) else {
        parsed.events.push(PublicEvent::UnsupportedControl {
            request: request_id,
            subtype: subtype.unwrap_or_else(|| "unknown".into()),
        });
        return;
    };
    let tool = field(body, "tool_name")
        .and_then(identifier)
        .unwrap_or_else(|| "unnamed-tool".into());
    let known = body
        .get("input")
        .map(|input| tool_detail(&tool, input))
        .unwrap_or_default();
    let detail = if known.is_empty() {
        field(body, "description")
            .map(|text| public_text(text, DETAIL_BYTES))
            .unwrap_or_default()
    } else {
        known
    };
    parsed.events.push(PublicEvent::InputRequest {
        request: request_id,
        tool_use: field(body, "tool_use_id").and_then(identifier),
        tool,
        detail,
    });
}

/// The provider's answer to one of this adapter's requests.
pub(super) fn response(value: &Value) -> Option<PublicEvent> {
    let body = value.get("response")?;
    let request = field(body, "request_id").and_then(identifier)?;
    Some(PublicEvent::Answered {
        request,
        ok: field(body, "subtype") == Some("success"),
    })
}
