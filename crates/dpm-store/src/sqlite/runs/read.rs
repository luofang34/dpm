//! Reading runs, lifecycle and activity in one consistent transaction.

use super::{
    RunStore,
    codec::{
        ACTIVITY_COLUMNS, LIFECYCLE_COLUMNS, LINK_COLUMNS, counter, read_activity, read_event,
        read_link, read_run,
    },
};
use crate::{
    RunStoreError, StoreError, StoredRecord,
    error::database_error,
    sqlite::snapshot::{decode, revision_to_sql},
};
use dpm_model::{
    ActivityEntry, ActivityGap, ActivityPage, ActivityTally, LatestActivity, LifecycleEntry,
    LifecyclePage, RunId, RunLink, RunRecord, RunSnapshot, WorkItemId,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

/// Most entries one feed page or run list returns.
pub(super) const PAGE_LIMIT: u16 = 1000;

pub(super) fn load_run(
    connection: &Connection,
    path: &Path,
    id: RunId,
) -> Result<Option<RunRecord>, StoreError> {
    connection
        .query_row(
            "SELECT record_json FROM runs WHERE run_id = ?1",
            params![id.to_string()],
            |row| Ok(read_run(row, path)),
        )
        .optional()
        .map_err(database_error(path, "look up run"))?
        .transpose()
}

pub(super) fn last_event(
    connection: &Connection,
    path: &Path,
    run: RunId,
) -> Result<Option<LifecycleEntry>, StoreError> {
    connection
        .query_row(
            &format!("{LIFECYCLE_COLUMNS} WHERE run_id = ?1 ORDER BY sequence DESC LIMIT 1"),
            params![run.to_string()],
            |row| Ok(read_event(row, path)),
        )
        .optional()
        .map_err(database_error(path, "look up run state"))?
        .transpose()
}

pub(super) fn event_by_id(
    connection: &Connection,
    path: &Path,
    id: dpm_model::RunEventId,
) -> Result<Option<LifecycleEntry>, StoreError> {
    connection
        .query_row(
            &format!("{LIFECYCLE_COLUMNS} WHERE event_id = ?1"),
            params![id.to_string()],
            |row| Ok(read_event(row, path)),
        )
        .optional()
        .map_err(database_error(path, "look up transition"))?
        .transpose()
}

pub(super) fn activity_by_sequence(
    connection: &Connection,
    path: &Path,
    run: RunId,
    source_sequence: u64,
) -> Result<Option<ActivityEntry>, StoreError> {
    connection
        .query_row(
            &format!("{ACTIVITY_COLUMNS} WHERE run_id = ?1 AND source_sequence = ?2"),
            params![run.to_string(), revision_to_sql(source_sequence)],
            |row| Ok(read_activity(row, path)),
        )
        .optional()
        .map_err(database_error(path, "look up activity"))?
        .transpose()
}

/// What a run's tally row holds: how many records it ever received and the highest source
/// sequence it accepted.
pub(super) struct TallyRow {
    pub(super) recorded: u64,
    pub(super) high_water: u64,
}

pub(super) fn tally_row(
    connection: &Connection,
    path: &Path,
    run: RunId,
) -> Result<Option<TallyRow>, StoreError> {
    connection
        .query_row(
            "SELECT recorded, high_water FROM run_activity_tally WHERE run_id = ?1",
            params![run.to_string()],
            |row| Ok(read_tally(row, path)),
        )
        .optional()
        .map_err(database_error(path, "read run tally"))?
        .transpose()
}

fn read_tally(row: &rusqlite::Row<'_>, path: &Path) -> Result<TallyRow, StoreError> {
    Ok(TallyRow {
        recorded: counter(row, 0, path, "recorded")?,
        high_water: counter(row, 1, path, "high_water")?,
    })
}

pub(super) fn link_by_operation(
    connection: &Connection,
    path: &Path,
    operation: dpm_model::OperationId,
) -> Result<Option<RunLink>, StoreError> {
    connection
        .query_row(
            &format!("{LINK_COLUMNS} WHERE operation_id = ?1"),
            params![operation.to_string()],
            |row| Ok(read_link(row, path)),
        )
        .optional()
        .map_err(database_error(path, "look up run link"))?
        .transpose()
}

fn links_of(connection: &Connection, path: &Path, run: RunId) -> Result<Vec<RunLink>, StoreError> {
    let mut statement = connection
        .prepare(&format!(
            "{LINK_COLUMNS} WHERE run_id = ?1 ORDER BY linked_at, operation_id"
        ))
        .map_err(database_error(path, "prepare run links"))?;
    let mut rows = statement
        .query(params![run.to_string()])
        .map_err(database_error(path, "read run links"))?;
    let mut links = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(database_error(path, "read run links"))?
    {
        links.push(read_link(row, path)?);
    }
    Ok(links)
}

fn tally(connection: &Connection, path: &Path, run: RunId) -> Result<ActivityTally, StoreError> {
    let retained: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM run_activity WHERE run_id = ?1",
            params![run.to_string()],
            |row| row.get(0),
        )
        .map_err(database_error(path, "count run activity"))?;
    let Some(counts) = tally_row(connection, path, run)? else {
        return Ok(ActivityTally {
            recorded: 0,
            retained,
            source_high_water: 0,
            latest: None,
        });
    };
    let (kind, at): (String, String) = connection
        .query_row(
            "SELECT last_kind, last_at FROM run_activity_tally WHERE run_id = ?1",
            params![run.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(database_error(path, "read latest activity"))?;
    let scalar = |value: String| serde_json::Value::String(value).to_string();
    Ok(ActivityTally {
        recorded: counts.recorded,
        retained,
        source_high_water: counts.high_water,
        latest: Some(LatestActivity {
            kind: decode(&scalar(kind), path, StoredRecord::Run, "last_kind")?,
            source_sequence: counts.high_water,
            recorded_at: decode(&scalar(at), path, StoredRecord::Run, "last_at")?,
        }),
    })
}

/// Everything the store holds about one run.
pub(super) fn snapshot(
    connection: &Connection,
    path: &Path,
    id: RunId,
) -> Result<Option<RunSnapshot>, StoreError> {
    let Some(record) = load_run(connection, path, id)? else {
        return Ok(None);
    };
    let last = last_event(connection, path, id)?.ok_or_else(|| StoreError::CorruptJson {
        path: path.to_path_buf(),
        record: StoredRecord::Run,
        field: "run_lifecycle",
        source: missing_start(id),
    })?;
    Ok(Some(RunSnapshot {
        operations: links_of(connection, path, id)?,
        activity: tally(connection, path, id)?,
        last: last.event,
        record,
    }))
}

/// A run row with no lifecycle row is damage; this reports it with the run it concerns.
fn missing_start(id: RunId) -> serde_json::Error {
    <serde_json::Error as serde::de::Error>::custom(format!("run {id} has no lifecycle record"))
}

impl RunStore {
    /// Where the feeds and the link count stand and the epoch they belong to, in one read
    /// transaction.
    pub fn heads_blocking(&self) -> Result<dpm_model::RunFeedHeads, RunStoreError> {
        let transaction = self.read_transaction()?;
        let binding = super::read_binding(&transaction, &self.path)?;
        Ok(dpm_model::RunFeedHeads {
            epoch: Some(binding.lineage_id),
            lifecycle_head: head(&transaction, &self.path, "run_lifecycle")?,
            activity_head: head(&transaction, &self.path, "run_activity")?,
            activity_pruned_through: pruned_through(&transaction, &self.path)?,
            link_count: link_count_blocking(&transaction, &self.path)?,
        })
    }

    /// Everything recorded about one run, or `None` for an unknown identity.
    pub fn snapshot_blocking(&self, id: RunId) -> Result<Option<RunSnapshot>, RunStoreError> {
        let transaction = self.read_transaction()?;
        Ok(snapshot(&transaction, &self.path, id)?)
    }

    /// Runs, newest first, optionally only those executing one task; at most 1000.
    pub fn snapshots_blocking(
        &self,
        work: Option<WorkItemId>,
        limit: u16,
    ) -> Result<Vec<RunSnapshot>, RunStoreError> {
        let transaction = self.read_transaction()?;
        let mut statement = transaction
            .prepare(
                "SELECT run_id FROM runs WHERE (?1 IS NULL OR work_id = ?1) \
                 ORDER BY rowid DESC LIMIT ?2",
            )
            .map_err(database_error(&self.path, "prepare run list"))?;
        let ids = statement
            .query_map(
                params![work.map(|id| id.to_string()), limit.min(PAGE_LIMIT)],
                |row| row.get::<_, String>(0),
            )
            .map_err(database_error(&self.path, "list runs"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error(&self.path, "read run list"))?;
        let mut runs = Vec::with_capacity(ids.len());
        for text in ids {
            let id: RunId = decode(
                &serde_json::Value::String(text).to_string(),
                &self.path,
                StoredRecord::Run,
                "run_id",
            )?;
            runs.extend(snapshot(&transaction, &self.path, id)?);
        }
        Ok(runs)
    }

    /// Lifecycle facts after a feed cursor, optionally only one run's; at most 1000. Lifecycle is
    /// never pruned, so a cursor is always continuous.
    pub fn lifecycle_page_blocking(
        &self,
        after_sequence: u64,
        limit: u16,
        run: Option<RunId>,
    ) -> Result<LifecyclePage, RunStoreError> {
        let transaction = self.read_transaction()?;
        let mut entries = Vec::new();
        if let Ok(cursor) = i64::try_from(after_sequence) {
            let mut statement = transaction
                .prepare(&format!(
                    "{LIFECYCLE_COLUMNS} WHERE sequence > ?1 AND (?2 IS NULL OR run_id = ?2) \
                     ORDER BY sequence LIMIT ?3"
                ))
                .map_err(database_error(&self.path, "prepare lifecycle feed"))?;
            let mut rows = statement
                .query(params![
                    cursor,
                    run.map(|id| id.to_string()),
                    limit.min(PAGE_LIMIT)
                ])
                .map_err(database_error(&self.path, "read lifecycle feed"))?;
            while let Some(row) = rows
                .next()
                .map_err(database_error(&self.path, "read lifecycle feed"))?
            {
                entries.push(read_event(row, &self.path)?);
            }
        }
        let head = head(&transaction, &self.path, "run_lifecycle")?;
        Ok(LifecyclePage {
            next_after_sequence: entries
                .last()
                .map_or(after_sequence, |entry| entry.sequence),
            entries,
            head_sequence: head,
        })
    }

    /// Activity after a feed cursor, optionally only one run's; at most 1000.
    ///
    /// A cursor older than what retention kept returns a [`ActivityGap`] and resumes from the
    /// oldest record still held, so a client is told the feed is discontinuous rather than served
    /// a silent hole.
    pub fn activity_page_blocking(
        &self,
        after_sequence: u64,
        limit: u16,
        run: Option<RunId>,
    ) -> Result<ActivityPage, RunStoreError> {
        let transaction = self.read_transaction()?;
        let pruned = pruned_through(&transaction, &self.path)?;
        let mut entries = Vec::new();
        if let Ok(cursor) = i64::try_from(after_sequence.max(pruned)) {
            let mut statement = transaction
                .prepare(&format!(
                    "{ACTIVITY_COLUMNS} WHERE sequence > ?1 AND (?2 IS NULL OR run_id = ?2) \
                     ORDER BY sequence LIMIT ?3"
                ))
                .map_err(database_error(&self.path, "prepare activity feed"))?;
            let mut rows = statement
                .query(params![
                    cursor,
                    run.map(|id| id.to_string()),
                    limit.min(PAGE_LIMIT)
                ])
                .map_err(database_error(&self.path, "read activity feed"))?;
            while let Some(row) = rows
                .next()
                .map_err(database_error(&self.path, "read activity feed"))?
            {
                entries.push(read_activity(row, &self.path)?);
            }
        }
        let gap = (after_sequence < pruned).then(|| ActivityGap {
            requested_after: after_sequence,
            resumes_at: pruned.saturating_add(1),
            lost: pruned.saturating_sub(after_sequence),
        });
        let resumed = gap.map_or(after_sequence, |_| pruned);
        Ok(ActivityPage {
            next_after_sequence: entries.last().map_or(resumed, |entry| entry.sequence),
            entries,
            head_sequence: head(&transaction, &self.path, "run_activity")?,
            gap,
        })
    }
}

/// The highest activity sequence retention has removed; zero while nothing was pruned.
pub(super) fn pruned_through(connection: &Connection, path: &Path) -> Result<u64, StoreError> {
    connection
        .query_row(
            "SELECT pruned_through FROM run_activity_retention WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(database_error(path, "read activity retention"))
}

/// How many operations were ever linked to a run; zero while none was.
///
/// `run_links` is append-only: a link is inserted once and never updated or removed, and a replayed
/// link inserts nothing. The count is therefore a monotone invalidation token. It deliberately
/// does not use the table's implicit `rowid`, which SQLite does not promise to keep stable and
/// which can be reused at its maximum, so it offers no order and no resumable position.
fn link_count_blocking(connection: &Connection, path: &Path) -> Result<u64, StoreError> {
    connection
        .query_row("SELECT COUNT(*) FROM run_links", [], |row| row.get(0))
        .map_err(database_error(path, "count run links"))
}

/// The newest sequence ever assigned in an `AUTOINCREMENT` table, even if retention removed it.
pub(super) fn head(
    connection: &Connection,
    path: &Path,
    table: &'static str,
) -> Result<u64, StoreError> {
    connection
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = ?1",
            params![table],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error(path, "read feed head"))
        .map(Option::unwrap_or_default)
}
