//! Reading run store rows into typed records, attributing damage to the field that holds it.

use crate::{
    StoreError, StoredRecord,
    sqlite::snapshot::{column, decode, revision_from_sql},
};
use chrono::{DateTime, Utc};
use dpm_model::{
    ActivityEntry, ActivityRecord, LifecycleEntry, LifecycleEvent, RunLink, RunRecord,
};
use rusqlite::Row;
use serde::de::DeserializeOwned;
use std::path::Path;

/// Selects the columns [`read_event`] expects, in its order.
pub(super) const LIFECYCLE_COLUMNS: &str = "SELECT sequence, event_id, run_id, state, detail, \
     observed_at, recorded_by_json, recorded_at FROM run_lifecycle";

/// Selects the columns [`read_activity`] expects, in its order.
pub(super) const ACTIVITY_COLUMNS: &str = "SELECT sequence, run_id, source_sequence, kind, text, \
     truncated, text_digest, observed_at, recorded_by_json, recorded_at FROM run_activity";

/// Selects the columns [`read_link`] expects, in its order.
pub(super) const LINK_COLUMNS: &str =
    "SELECT operation_id, run_id, work_id, linked_by_json, linked_at FROM run_links";

/// An identifier, state word or timestamp column, which holds a bare string; decoding it as a JSON
/// string reuses the serde format the type itself declares.
fn scalar<T: DeserializeOwned>(
    row: &Row<'_>,
    index: usize,
    path: &Path,
    field: &'static str,
) -> Result<T, StoreError> {
    let text: String = column(row, index, path, StoredRecord::Run, field)?;
    decode(
        &serde_json::Value::String(text).to_string(),
        path,
        StoredRecord::Run,
        field,
    )
}

fn optional_scalar<T: DeserializeOwned>(
    row: &Row<'_>,
    index: usize,
    path: &Path,
    field: &'static str,
) -> Result<Option<T>, StoreError> {
    let text: Option<String> = column(row, index, path, StoredRecord::Run, field)?;
    text.map(|text| {
        decode(
            &serde_json::Value::String(text).to_string(),
            path,
            StoredRecord::Run,
            field,
        )
    })
    .transpose()
}

/// A column holding JSON text.
pub(super) fn json<T: DeserializeOwned>(
    row: &Row<'_>,
    index: usize,
    path: &Path,
    field: &'static str,
) -> Result<T, StoreError> {
    let text: String = column(row, index, path, StoredRecord::Run, field)?;
    decode(&text, path, StoredRecord::Run, field)
}

/// The storage form of a timestamp, which round-trips exactly.
pub(super) fn time_text(at: DateTime<Utc>) -> String {
    at.to_rfc3339()
}

pub(super) fn optional_time_text(at: Option<DateTime<Utc>>) -> Option<String> {
    at.map(time_text)
}

/// A counter stored bit-for-bit in an `INTEGER`, so the whole `u64` range round-trips.
pub(super) fn counter(
    row: &Row<'_>,
    index: usize,
    path: &Path,
    field: &'static str,
) -> Result<u64, StoreError> {
    column::<i64>(row, index, path, StoredRecord::Run, field).map(revision_from_sql)
}

fn sequence(row: &Row<'_>, path: &Path) -> Result<u64, StoreError> {
    column(row, 0, path, StoredRecord::Run, "sequence")
}

pub(super) fn read_event(row: &Row<'_>, path: &Path) -> Result<LifecycleEntry, StoreError> {
    Ok(LifecycleEntry {
        sequence: sequence(row, path)?,
        event: LifecycleEvent {
            id: scalar(row, 1, path, "event_id")?,
            run: scalar(row, 2, path, "run_id")?,
            state: scalar(row, 3, path, "state")?,
            detail: column(row, 4, path, StoredRecord::Run, "detail")?,
            observed_at: optional_scalar(row, 5, path, "observed_at")?,
            recorded_by: json(row, 6, path, "recorded_by_json")?,
            recorded_at: scalar(row, 7, path, "recorded_at")?,
        },
    })
}

pub(super) fn read_activity(row: &Row<'_>, path: &Path) -> Result<ActivityEntry, StoreError> {
    Ok(ActivityEntry {
        sequence: sequence(row, path)?,
        record: ActivityRecord {
            run: scalar(row, 1, path, "run_id")?,
            source_sequence: counter(row, 2, path, "source_sequence")?,
            kind: scalar(row, 3, path, "kind")?,
            text: column(row, 4, path, StoredRecord::Run, "text")?,
            truncated: column(row, 5, path, StoredRecord::Run, "truncated")?,
            text_digest: column(row, 6, path, StoredRecord::Run, "text_digest")?,
            observed_at: optional_scalar(row, 7, path, "observed_at")?,
            recorded_by: json(row, 8, path, "recorded_by_json")?,
            recorded_at: scalar(row, 9, path, "recorded_at")?,
        },
    })
}

pub(super) fn read_link(row: &Row<'_>, path: &Path) -> Result<RunLink, StoreError> {
    Ok(RunLink {
        operation: scalar(row, 0, path, "operation_id")?,
        run: scalar(row, 1, path, "run_id")?,
        work: scalar(row, 2, path, "work_id")?,
        linked_by: json(row, 3, path, "linked_by_json")?,
        linked_at: scalar(row, 4, path, "linked_at")?,
    })
}

pub(super) fn read_run(row: &Row<'_>, path: &Path) -> Result<RunRecord, StoreError> {
    json(row, 0, path, "record_json")
}
