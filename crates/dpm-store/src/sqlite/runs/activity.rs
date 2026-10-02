//! Recording activity: identity by reporter sequence, bounded retention, and the receipt tally.

use super::{
    Written,
    codec::{optional_time_text, time_text},
    read::{TallyRow, activity_by_sequence, pruned_through, tally_row},
    write::{corrupt_sequence, to_json},
};
use crate::{RunStoreError, StoreError, error::database_error, sqlite::snapshot::revision_to_sql};
use chrono::{DateTime, Utc};
use dpm_model::{ActivityEntry, ActivityInput, ActivityRecord, ActorId};
use rusqlite::{Transaction, params};
use std::path::Path;

pub(super) fn append_one(
    transaction: &Transaction<'_>,
    path: &Path,
    input: &ActivityInput,
    writer: &ActorId,
    now: DateTime<Utc>,
) -> Result<Written<ActivityEntry>, RunStoreError> {
    let normalized = input.normalized();
    if let Some(recorded) =
        activity_by_sequence(transaction, path, input.run, input.source_sequence)?
    {
        let held = &recorded.record;
        let same = held.kind == input.kind
            && held.text == normalized.text
            && held.truncated == normalized.truncated
            && held.text_digest == normalized.text_digest
            && held.observed_at == input.observed_at
            && held.recorded_by == *writer;
        return if same {
            Ok(Written {
                value: recorded,
                replayed: true,
            })
        } else {
            Err(RunStoreError::DuplicateActivity {
                recorded: Box::new(recorded),
            })
        };
    }
    let counts = tally_row(transaction, path, input.run)?.unwrap_or(TallyRow {
        recorded: 0,
        high_water: 0,
    });
    if input.source_sequence <= counts.high_water {
        return Err(RunStoreError::ActivityExpired {
            run: input.run,
            source_sequence: input.source_sequence,
            high_water: counts.high_water,
        });
    }
    let record = ActivityRecord {
        run: input.run,
        source_sequence: input.source_sequence,
        kind: input.kind,
        text: normalized.text,
        truncated: normalized.truncated,
        text_digest: normalized.text_digest,
        observed_at: input.observed_at,
        recorded_by: writer.clone(),
        recorded_at: now,
    };
    let sequence = insert(transaction, path, &record)?;
    bump_tally(transaction, path, &record, &counts)?;
    Ok(Written {
        value: ActivityEntry { sequence, record },
        replayed: false,
    })
}

fn insert(
    transaction: &Transaction<'_>,
    path: &Path,
    record: &ActivityRecord,
) -> Result<u64, RunStoreError> {
    transaction
        .execute(
            "INSERT INTO run_activity(run_id, source_sequence, kind, text, truncated, text_digest, \
             observed_at, recorded_by_json, recorded_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.run.to_string(),
                revision_to_sql(record.source_sequence),
                record.kind.word(),
                record.text,
                record.truncated,
                record.text_digest,
                optional_time_text(record.observed_at),
                to_json(&record.recorded_by)?,
                time_text(record.recorded_at),
            ],
        )
        .map_err(database_error(path, "append activity"))?;
    Ok(u64::try_from(transaction.last_insert_rowid()).map_err(|_| corrupt_sequence(path))?)
}

fn bump_tally(
    transaction: &Transaction<'_>,
    path: &Path,
    record: &ActivityRecord,
    counts: &TallyRow,
) -> Result<(), RunStoreError> {
    // The receipt count is a monotonic counter and wraps; the high-water mark only ever rises.
    transaction
        .execute(
            "INSERT INTO run_activity_tally(run_id, recorded, last_kind, high_water, last_at) \
             VALUES(?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(run_id) DO UPDATE SET recorded = excluded.recorded, \
             last_kind = excluded.last_kind, high_water = excluded.high_water, \
             last_at = excluded.last_at",
            params![
                record.run.to_string(),
                revision_to_sql(counts.recorded.wrapping_add(1)),
                record.kind.word(),
                revision_to_sql(record.source_sequence),
                time_text(record.recorded_at),
            ],
        )
        .map_err(database_error(path, "update activity tally"))?;
    Ok(())
}

/// Keep at most `limit` of the newest activity records and remember what was removed.
///
/// Sequences are contiguous and never reused, so the records held are exactly those above
/// `pruned_through`, and a cursor below it is known to have missed something.
pub(super) fn prune(
    transaction: &Transaction<'_>,
    path: &Path,
    limit: u64,
) -> Result<(), StoreError> {
    let (oldest, newest): (Option<i64>, Option<i64>) = transaction
        .query_row(
            "SELECT MIN(sequence), MAX(sequence) FROM run_activity",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(database_error(path, "measure activity"))?;
    let (Some(oldest), Some(newest)) = (oldest, newest) else {
        return Ok(());
    };
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let held = newest.saturating_sub(oldest).saturating_add(1);
    if held <= limit {
        return Ok(());
    }
    let through = newest.saturating_sub(limit);
    transaction
        .execute(
            "DELETE FROM run_activity WHERE sequence <= ?1",
            params![through],
        )
        .map_err(database_error(path, "prune activity"))?;
    let already = i64::try_from(pruned_through(transaction, path)?).unwrap_or(i64::MAX);
    transaction
        .execute(
            "UPDATE run_activity_retention SET pruned_through = ?1 WHERE singleton = 1",
            params![through.max(already)],
        )
        .map_err(database_error(path, "record pruned activity"))?;
    tracing::debug!(through, "pruned run activity beyond retention");
    Ok(())
}
