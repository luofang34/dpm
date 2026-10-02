//! Backup, restore and read-only verification of a run store.
//!
//! The run store is not part of the project store's backup file, so the project store's backup,
//! restore and verification each carry it explicitly: a backup writes `<backup>.runs` beside the
//! project backup, a restore copies it into `<store>.runs` bound to the restored lineage, and
//! verification checks it. Runs recorded under an earlier lineage keep their attribution; they are
//! reported as foreign, never reattached.

use super::{
    RunBinding, RunStore, check_workspace,
    codec::{read_activity, read_event, read_link, read_run},
    pairing::{PairFacts, read_forks},
    read::{head, pruned_through, tally_row},
    read_binding, schema,
};
use crate::{
    LineageError, RunStoreError, StoreError,
    error::database_error,
    schema::Layout,
    sqlite::{
        integrity::integrity_check_blocking,
        recovery::{
            copy_blocking,
            files::{
                ReadMode, canonical_blocking, create_target_blocking, discard_blocking,
                fingerprint_blocking, open_read_only_blocking, read_mode_blocking,
            },
        },
        snapshot::revision_to_sql,
    },
};
use chrono::{DateTime, Utc};
use dpm_engine::{check_run_transition, run_start_event};
use dpm_model::{LineageId, RunId, RunLink, RunState, WorkspaceId};
use rusqlite::Connection;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// What a verified run store holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunStoreReport {
    /// Verified file as a canonical absolute path.
    pub path: PathBuf,
    /// Layout version in the header.
    pub schema_version: i64,
    /// Workspace the run store is bound to.
    pub workspace_id: WorkspaceId,
    /// History the run store was created or last restored for; runs recorded earlier are foreign.
    pub lineage_id: LineageId,
    /// Whether the file is a backup that only a restore makes writable.
    pub archived: bool,
    /// Runs recorded.
    pub runs: u64,
    /// Lifecycle facts recorded; they are never pruned.
    pub lifecycle_events: u64,
    /// Activity records still held.
    pub activity_retained: u64,
    /// Highest activity sequence retention has removed.
    pub activity_pruned_through: u64,
}

pub(super) fn corrupt(path: &Path, detail: impl Into<String>) -> RunStoreError {
    RunStoreError::Corrupt {
        path: path.to_path_buf(),
        detail: detail.into(),
    }
}

/// Verify a run store without writing to it or creating any file next to it.
pub(in crate::sqlite) fn verify_blocking(path: &Path) -> Result<RunStoreReport, RunStoreError> {
    verify_with_facts_blocking(path).map(|(report, _)| report)
}

/// Verify a run store and keep what it claims about its project store.
pub(in crate::sqlite) fn verify_with_facts_blocking(
    path: &Path,
) -> Result<(RunStoreReport, PairFacts), RunStoreError> {
    let path = canonical_blocking(path)?;
    if read_mode_blocking(&path)? == ReadMode::Immutable {
        let before = fingerprint_blocking(&path)?;
        let result = verify_with_blocking(&path, ReadMode::Immutable);
        // A writer that started meanwhile may have changed pages under the unlocked read.
        if fingerprint_blocking(&path)? == before {
            return result;
        }
    }
    verify_with_blocking(&path, ReadMode::Shared)
}

fn verify_with_blocking(
    path: &Path,
    mode: ReadMode,
) -> Result<(RunStoreReport, PairFacts), RunStoreError> {
    let mut connection = open_read_only_blocking(path, mode)?;
    let transaction = connection
        .transaction()
        .map_err(database_error(path, "begin run store verification"))?;
    if schema::check_blocking(&transaction, path)? == Layout::Empty {
        return Err(StoreError::UnrecognizedSchema {
            path: path.to_path_buf(),
            detail: "no run store tables".into(),
        }
        .into());
    }
    integrity_check_blocking(&transaction, path)?;
    let binding = read_binding(&transaction, path)?;
    let runs = check_runs(&transaction, path)?;
    let (lifecycle_events, last) = check_lifecycle(&transaction, path, &runs)?;
    let (activity_retained, pruned) = check_activity(&transaction, path, &runs)?;
    let links = check_links(&transaction, path, &runs)?;
    let report = RunStoreReport {
        path: path.to_path_buf(),
        schema_version: schema::RUN_STORE_VERSION,
        workspace_id: binding.workspace_id,
        lineage_id: binding.lineage_id,
        archived: binding.archived,
        runs: u64::try_from(runs.len()).unwrap_or(u64::MAX),
        lifecycle_events,
        activity_retained,
        activity_pruned_through: pruned,
    };
    let runs = runs
        .into_values()
        .map(|record| {
            let ended = last
                .get(&record.id)
                .filter(|(state, _)| state.is_terminal())
                .map(|(_, at)| *at);
            (record, ended)
        })
        .collect();
    let facts = PairFacts {
        path: path.to_path_buf(),
        binding,
        runs,
        links,
        forks: read_forks(&transaction, path)?,
    };
    Ok((report, facts))
}

/// Every run decodes and sits under its own identity; returns each one's task.
fn check_runs(
    connection: &Connection,
    path: &Path,
) -> Result<BTreeMap<RunId, dpm_model::RunRecord>, RunStoreError> {
    let mut statement = connection
        .prepare("SELECT record_json, run_id, work_id FROM runs")
        .map_err(database_error(path, "prepare run verification"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "read runs"))?;
    let mut runs = BTreeMap::new();
    while let Some(row) = rows.next().map_err(database_error(path, "read runs"))? {
        let record = read_run(row, path)?;
        let (id, work): (String, String) = (
            row.get(1).map_err(database_error(path, "read run id"))?,
            row.get(2).map_err(database_error(path, "read run task"))?,
        );
        if record.id.to_string() != id || record.work.to_string() != work {
            return Err(corrupt(
                path,
                format!("run row {id} disagrees with its record"),
            ));
        }
        runs.insert(record.id, record);
    }
    Ok(runs)
}

/// Each run's last lifecycle state and when it was recorded.
type LastStates = BTreeMap<RunId, (RunState, DateTime<Utc>)>;

/// Lifecycle is contiguous, starts every run at its own identity and obeys the state machine.
fn check_lifecycle(
    connection: &Connection,
    path: &Path,
    runs: &BTreeMap<RunId, dpm_model::RunRecord>,
) -> Result<(u64, LastStates), RunStoreError> {
    let mut statement = connection
        .prepare(&format!(
            "{} ORDER BY sequence",
            super::codec::LIFECYCLE_COLUMNS
        ))
        .map_err(database_error(path, "prepare lifecycle verification"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "read lifecycle"))?;
    let mut states = LastStates::new();
    let mut previous = 0_u64;
    while let Some(row) = rows
        .next()
        .map_err(database_error(path, "read lifecycle"))?
    {
        let entry = read_event(row, path)?;
        if entry.sequence != previous.wrapping_add(1) {
            return Err(corrupt(
                path,
                format!(
                    "lifecycle skips from sequence {previous} to {}",
                    entry.sequence
                ),
            ));
        }
        previous = entry.sequence;
        let event = entry.event;
        let Some(record) = runs.get(&event.run) else {
            return Err(corrupt(
                path,
                format!("lifecycle names unknown run {}", event.run),
            ));
        };
        match states.get(&event.run) {
            None => {
                let begins = event.id == run_start_event(event.run)
                    && event.state == RunState::Working
                    && event.recorded_at == record.started_at;
                if !begins {
                    return Err(corrupt(
                        path,
                        format!("run {} does not begin with its start", event.run),
                    ));
                }
            }
            Some((from, _)) => {
                check_run_transition(event.run, *from, event.state).map_err(|error| {
                    corrupt(path, format!("illegal transition recorded: {error}"))
                })?;
            }
        }
        states.insert(event.run, (event.state, event.recorded_at));
    }
    if let Some(unstarted) = runs.keys().find(|id| !states.contains_key(id)) {
        return Err(corrupt(
            path,
            format!("run {unstarted} has no lifecycle record"),
        ));
    }
    if head(connection, path, "run_lifecycle")? != previous {
        return Err(corrupt(path, "lifecycle records were removed from the end"));
    }
    Ok((previous, states))
}

/// Retained activity is contiguous above what retention removed, and tallies cover what is held.
fn check_activity(
    connection: &Connection,
    path: &Path,
    runs: &BTreeMap<RunId, dpm_model::RunRecord>,
) -> Result<(u64, u64), RunStoreError> {
    let pruned = pruned_through(connection, path)?;
    let mut statement = connection
        .prepare(&format!(
            "{} ORDER BY sequence",
            super::codec::ACTIVITY_COLUMNS
        ))
        .map_err(database_error(path, "prepare activity verification"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "read activity"))?;
    let (mut previous, mut retained) = (None::<u64>, 0_u64);
    let mut per_run: BTreeMap<RunId, u64> = BTreeMap::new();
    let mut newest_source: BTreeMap<RunId, u64> = BTreeMap::new();
    while let Some(row) = rows.next().map_err(database_error(path, "read activity"))? {
        let entry = read_activity(row, path)?;
        let expected = previous.map_or(pruned.wrapping_add(1), |last| last.wrapping_add(1));
        if entry.sequence != expected {
            return Err(corrupt(
                path,
                format!(
                    "activity holds sequence {} where {expected} should follow",
                    entry.sequence
                ),
            ));
        }
        if !runs.contains_key(&entry.record.run) {
            return Err(corrupt(
                path,
                format!("activity names unknown run {}", entry.record.run),
            ));
        }
        previous = Some(entry.sequence);
        retained = retained.wrapping_add(1);
        let held = per_run.entry(entry.record.run).or_default();
        *held = held.wrapping_add(1);
        let source = entry.record.source_sequence;
        if newest_source
            .insert(entry.record.run, source)
            .is_some_and(|earlier| earlier >= source)
        {
            return Err(corrupt(
                path,
                format!(
                    "run {} holds source sequence {source} after a later or equal one",
                    entry.record.run
                ),
            ));
        }
    }
    if head(connection, path, "run_activity")? != pruned.wrapping_add(retained) {
        return Err(corrupt(
            path,
            "activity records were removed outside retention",
        ));
    }
    // The receipt count is a wrapping counter, so only the high-water mark is checked: it must
    // exist for every run that holds activity and may not sit below a sequence that was accepted.
    for (run, newest) in newest_source {
        let high_water = tally_row(connection, path, run)?.map(|counts| counts.high_water);
        if high_water.is_none_or(|high_water| high_water < newest) {
            return Err(corrupt(
                path,
                format!("run {run} holds source sequence {newest} above its high-water mark"),
            ));
        }
    }
    Ok((retained, pruned))
}

fn check_links(
    connection: &Connection,
    path: &Path,
    runs: &BTreeMap<RunId, dpm_model::RunRecord>,
) -> Result<Vec<RunLink>, RunStoreError> {
    let mut statement = connection
        .prepare(&format!(
            "{} ORDER BY operation_id",
            super::codec::LINK_COLUMNS
        ))
        .map_err(database_error(path, "prepare link verification"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "read links"))?;
    let mut links = Vec::new();
    while let Some(row) = rows.next().map_err(database_error(path, "read links"))? {
        let link = read_link(row, path)?;
        match runs.get(&link.run) {
            Some(record) if record.work == link.work => {}
            _ => {
                return Err(corrupt(
                    path,
                    format!(
                        "operation {} is linked to a run that does not match it",
                        link.operation
                    ),
                ));
            }
        }
        links.push(link);
    }
    Ok(links)
}

/// What a copied run store becomes before it is closed.
#[derive(Clone, Copy)]
enum Seal {
    Archive,
    /// A restore: the copy continues as `lineage`, starting at the project's `revision`.
    Fork {
        lineage: LineageId,
        revision: u64,
    },
}

fn seal_blocking(target: &Connection, path: &Path, seal: Seal) -> Result<(), StoreError> {
    let changed = match seal {
        Seal::Archive => target.execute("UPDATE run_binding SET archived = 1", []),
        Seal::Fork { lineage, revision } => {
            // The fork is recorded as explicit evidence, so the runs of the lineage it continued
            // can still be traced to the history that holds their observed revisions.
            let parent: String = target
                .query_row("SELECT lineage_id FROM run_binding", [], |row| row.get(0))
                .map_err(database_error(path, "read the run store's lineage"))?;
            target
                .execute(
                    "INSERT INTO run_lineage_forks(lineage_id, parent_lineage_id, revision) \
                     VALUES(?1, ?2, ?3)",
                    rusqlite::params![lineage.to_string(), parent, revision_to_sql(revision)],
                )
                .map_err(database_error(path, "record the lineage fork"))?;
            target.execute(
                "UPDATE run_binding SET lineage_id = ?1, archived = 0",
                [lineage.to_string()],
            )
        }
    }
    .map_err(database_error(path, "seal copied run store"))?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StoreError::Lineage(LineageError::Missing {
            path: path.to_path_buf(),
        }))
    }
}

impl RunStore {
    /// Copy this run store into a new file, sealed as an archive of its binding, and verify the
    /// copy. The target is created by the caller and removed again if this fails.
    pub(in crate::sqlite) fn backup_to_blocking(
        &self,
        to: &Path,
    ) -> Result<RunStoreReport, RunStoreError> {
        copy_blocking(&self.connection, to, "delete", |target| {
            seal_blocking(target, to, Seal::Archive)
        })?;
        verify_blocking(to)
    }
}

/// Copy a verified run archive into a new run store bound to `lineage`, the lineage of the
/// restored project store, which starts at project `revision`, and verify the copy. The target is created by the caller.
///
/// Runs in the archive keep the lineage they were recorded under; they read as foreign in the
/// restored store, so nothing is silently reattached to the new history.
pub(in crate::sqlite) fn restore_blocking(
    from: &Path,
    to: &Path,
    workspace: WorkspaceId,
    lineage: LineageId,
    revision: u64,
) -> Result<RunStoreReport, RunStoreError> {
    let from = canonical_blocking(from)?;
    let report = verify_blocking(&from)?;
    if !report.archived {
        return Err(StoreError::from(LineageError::NotAnArchive { path: from }).into());
    }
    check_workspace(
        &RunBinding {
            workspace_id: report.workspace_id,
            lineage_id: report.lineage_id,
            archived: report.archived,
        },
        workspace,
        &from,
    )?;
    let mode = read_mode_blocking(&from)?;
    let before = fingerprint_blocking(&from)?;
    let source = open_read_only_blocking(&from, mode)?;
    copy_blocking(&source, to, "wal", |target| {
        seal_blocking(target, to, Seal::Fork { lineage, revision })
    })?;
    // An immutable read cannot see a writer that started meanwhile, so the copy is only trusted
    // if the source provably did not change.
    if mode == ReadMode::Immutable && fingerprint_blocking(&from)? != before {
        return Err(StoreError::BackupIncomplete {
            path: to.to_path_buf(),
            state: format!("{} changed during the restore", from.display()),
        }
        .into());
    }
    verify_blocking(to)
}

/// Create a run store backup target exclusively, as the project backup creates its own.
pub(in crate::sqlite) fn create_target(to: &Path) -> Result<(), StoreError> {
    create_target_blocking(to)
}

/// Remove a run store target this command created after a failure.
pub(in crate::sqlite) fn discard(to: &Path) {
    discard_blocking(to);
}
