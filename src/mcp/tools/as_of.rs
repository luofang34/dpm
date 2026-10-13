//! `as_of`: a query tool's look back at the workspace as it stood at an earlier instant, the
//! per-call form of the CLI's `--clock`.

use chrono::{DateTime, Utc};
use dpm_app::{AppError, Application, QueryClock};
use serde_json::{Map, Value, json};

/// Tools whose answers depend on the clock reading or on the revision it observes.
const TOOLS: [&str; 7] = [
    "project_status",
    "get_schedule",
    "get_calibration",
    "next_work",
    "get_work",
    "explain_work",
    "export_plan",
];

pub(super) fn schema(name: &str, properties: &mut Map<String, Value>) {
    if TOOLS.contains(&name) {
        properties.insert(
            "as_of".into(),
            json!({"type":"string","format":"date-time","description":"RFC 3339 instant: observe the workspace as it stood then, judged by when events occurred (states, gates, progress and forecasts of that moment; revision and structure stay current), like the CLI's --clock; defaults to the server's clock"}),
        );
    }
}

/// Remove `as_of` from a call's arguments.
pub(super) fn take(value: &mut Value) -> Result<Option<DateTime<Utc>>, AppError> {
    let Some(at) = value
        .as_object_mut()
        .and_then(|fields| fields.remove("as_of"))
    else {
        return Ok(None);
    };
    serde_json::from_value(at).map(Some).map_err(|error| {
        AppError::InvalidRequest(format!("as_of must be an RFC 3339 instant: {error}"))
    })
}

/// Run one call with queries pinned to `at`, restoring the server's clock whatever its outcome,
/// so the reading never leaks into later calls.
pub(super) fn observe_blocking<T>(
    app: &mut Application,
    at: DateTime<Utc>,
    call: impl FnOnce(&mut Application) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let server = app.query_clock();
    app.set_query_clock(QueryClock::Pinned(at));
    let answer = call(app);
    app.set_query_clock(server);
    answer
}
