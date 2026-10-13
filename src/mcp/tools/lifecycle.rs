//! Claim, start, submit, verify and decide, which may record when their event actually occurred.

use chrono::{DateTime, Utc};
use dpm_engine::Command;
use dpm_model::WorkItemId;
use serde_json::{Map, Value, json};

/// Work tools whose event time may be backfilled with the optional `at` argument; `decide_gate`
/// takes it too, beside its decision arguments.
pub(super) fn handles(name: &str) -> bool {
    matches!(
        name,
        "claim_work" | "start_work" | "submit_work" | "verify_work"
    )
}

pub(super) fn schema(name: &str, properties: &mut Map<String, Value>) {
    if matches!(name, "submit_work" | "verify_work") {
        properties.insert("note".into(), json!({"type":"string"}));
    }
    let description = if name == "decide_gate" {
        "RFC 3339 time the decision was actually made, when recording it after the fact; not after the commit. The operation keeps its own commit timestamp"
    } else {
        "RFC 3339 time the event actually occurred, when recording it after the fact; not after the commit and not before the task's latest recorded event. The operation keeps its own commit timestamp"
    };
    properties.insert(
        "at".into(),
        json!({"type":"string","format":"date-time","description":description}),
    );
}

/// The lifecycle command for a tool this module handles; `None` for any other name.
pub(super) fn command(
    name: &str,
    work: WorkItemId,
    note: Option<String>,
    occurred_at: Option<DateTime<Utc>>,
) -> Option<Command> {
    Some(match name {
        "claim_work" => Command::Claim { work, occurred_at },
        "start_work" => Command::Start { work, occurred_at },
        "submit_work" => Command::Submit {
            work,
            note,
            occurred_at,
        },
        "verify_work" => Command::Verify {
            work,
            note,
            occurred_at,
        },
        _ => return None,
    })
}
