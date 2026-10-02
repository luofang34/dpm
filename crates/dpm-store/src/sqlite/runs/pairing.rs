//! Checking that a run store and a project store form one consistent pair.
//!
//! The two files are copied at different moments, so a run store can claim facts its project store
//! does not hold. The check runs inside the project store's read transaction and refuses a pair
//! whose run store is bound to another history, observed a revision the history does not hold, or
//! links an operation the history lacks or that no longer passes the link rules.

use super::{RunBinding, codec::counter, recovery::corrupt};
use crate::{
    RunStoreError, StoreError, StoredRecord,
    error::database_error,
    sqlite::{
        history::recorded_blocking,
        snapshot::{column, decode, revision_to_sql},
    },
};
use chrono::{DateTime, Utc};
use dpm_engine::{OperationFacts, check_link};
use dpm_model::{LineageId, RunLink, RunRecord, WorkspaceId};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Where a lineage a restore created forked from.
#[derive(Debug, Clone, Copy)]
pub(in crate::sqlite) struct LineageFork {
    /// The lineage the restore created.
    pub(in crate::sqlite) lineage: LineageId,
    /// The lineage it continued.
    pub(in crate::sqlite) parent: LineageId,
    /// The project revision it started at.
    pub(in crate::sqlite) revision: u64,
}

/// What a run store claims about the project store beside it, kept so the pair can be checked
/// against the project history in the project store's own read transaction.
pub(in crate::sqlite) struct PairFacts {
    /// The verified run store file.
    pub(super) path: PathBuf,
    /// Its binding.
    pub(super) binding: RunBinding,
    /// Every run with the time it ended, if it did.
    pub(super) runs: Vec<(RunRecord, Option<DateTime<Utc>>)>,
    /// Every operation link.
    pub(super) links: Vec<RunLink>,
    /// Every lineage fork a restore recorded.
    pub(super) forks: Vec<LineageFork>,
}

/// The project store's identity as the pair check needs it.
pub(in crate::sqlite) struct ProjectIdentity {
    /// Workspace of the project store.
    pub(in crate::sqlite) workspace: WorkspaceId,
    /// Lineage the project store continues.
    pub(in crate::sqlite) lineage: LineageId,
    /// Revision of its snapshot.
    pub(in crate::sqlite) snapshot_revision: u64,
    /// Revision of its genesis plan.
    pub(in crate::sqlite) genesis_revision: u64,
}

/// Read the lineage forks a run store holds.
pub(super) fn read_forks(
    connection: &Connection,
    path: &Path,
) -> Result<Vec<LineageFork>, StoreError> {
    let mut statement = connection
        .prepare("SELECT lineage_id, parent_lineage_id, revision FROM run_lineage_forks")
        .map_err(database_error(path, "prepare lineage forks"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "read lineage forks"))?;
    let scalar = |text: String| serde_json::Value::String(text).to_string();
    let mut forks = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(database_error(path, "read lineage forks"))?
    {
        let text = |index, field| -> Result<String, StoreError> {
            column::<String>(row, index, path, StoredRecord::Run, field)
        };
        forks.push(LineageFork {
            lineage: decode(
                &scalar(text(0, "lineage_id")?),
                path,
                StoredRecord::Run,
                "lineage_id",
            )?,
            parent: decode(
                &scalar(text(1, "parent_lineage_id")?),
                path,
                StoredRecord::Run,
                "parent_lineage_id",
            )?,
            revision: counter(row, 2, path, "revision")?,
        });
    }
    Ok(forks)
}

/// Check that a run store is a self-consistent pair with the project store it sits beside.
///
/// The run store must be bound to the project's workspace and current lineage; every run must
/// have observed a plan revision the project history holds for the run's lineage; and every
/// operation link must name an operation that is in the project history and still passes the
/// link rules against it. A run store copied at a later moment than the project store it is paired
/// with fails here, because it attributes operations that the older project copy does not hold.
pub(in crate::sqlite) fn cross_check_blocking(
    project: &Connection,
    project_path: &Path,
    own: &ProjectIdentity,
    pair: &PairFacts,
) -> Result<(), RunStoreError> {
    let path = pair.path.as_path();
    if pair.binding.workspace_id != own.workspace {
        return Err(RunStoreError::WorkspaceMismatch {
            path: path.to_path_buf(),
            expected: own.workspace,
            actual: pair.binding.workspace_id,
        });
    }
    if pair.binding.lineage_id != own.lineage {
        return Err(corrupt(
            path,
            format!(
                "it is bound to lineage {}, but the project store continues lineage {}",
                pair.binding.lineage_id, own.lineage
            ),
        ));
    }
    for (record, _) in &pair.runs {
        let held = revision_held(
            project,
            project_path,
            own,
            &pair.forks,
            record.contract.lineage_id,
            record.contract.revision,
            0,
        )?;
        if record.contract.workspace_id != own.workspace || !held {
            return Err(corrupt(
                path,
                format!(
                    "run {} observed revision {} of lineage {}, which the project history does not hold",
                    record.id, record.contract.revision, record.contract.lineage_id
                ),
            ));
        }
    }
    for link in &pair.links {
        let Some(entry) = recorded_blocking(project, project_path, link.operation)? else {
            return Err(corrupt(
                path,
                format!(
                    "run {} is linked to operation {}, which the project history does not hold",
                    link.run, link.operation
                ),
            ));
        };
        let Some((record, ended)) = pair.runs.iter().find(|(record, _)| record.id == link.run)
        else {
            continue;
        };
        let recorded = &entry.operation;
        let facts = OperationFacts::of(
            &recorded.operation,
            recorded.workspace_id,
            recorded.lineage_id,
        );
        check_link(record, *ended, &facts).map_err(|error| {
            corrupt(
                path,
                format!(
                    "link of operation {} to run {} fails its rules: {error}",
                    link.operation, link.run
                ),
            )
        })?;
    }
    Ok(())
}

fn flag(
    project: &Connection,
    path: &Path,
    sql: &str,
    params: impl rusqlite::Params,
    action: &'static str,
) -> Result<bool, StoreError> {
    project
        .query_row(sql, params, |row| row.get(0))
        .map_err(database_error(path, action))
}

/// Whether the project history, with the forks the run store recorded, holds `revision` as a
/// state of `lineage`.
///
/// Revisions wrap, so this is membership and never numeric order. The revision is held when it is
/// the genesis revision; or, for the lineage the store continues, its snapshot; or the result of an
/// operation recorded under that lineage, or the revision the lineage's first operation started
/// from. A lineage a restore created may have no operations of its own, so it is also held through
/// the fork the restore recorded: the lineage started at the fork's revision, and that revision
/// must itself be held in the lineage it forked from. A lineage with no operations at all that
/// some other lineage forked from stayed at the revision of that fork, which is the only state
/// its runs could have observed.
fn revision_held(
    project: &Connection,
    project_path: &Path,
    own: &ProjectIdentity,
    forks: &[LineageFork],
    lineage: LineageId,
    revision: u64,
    depth: usize,
) -> Result<bool, RunStoreError> {
    if revision == own.genesis_revision
        || (lineage == own.lineage && revision == own.snapshot_revision)
    {
        return Ok(true);
    }
    let (name, wanted) = (lineage.to_string(), revision_to_sql(revision));
    let resulted = flag(
        project,
        project_path,
        "SELECT EXISTS(SELECT 1 FROM operations WHERE lineage_id = ?1 AND resulting_revision = ?2)",
        rusqlite::params![name, wanted],
        "read lineage revisions",
    )?;
    let started = flag(
        project,
        project_path,
        "SELECT EXISTS(SELECT 1 FROM (SELECT base_revision FROM operations WHERE lineage_id = ?1 \
         ORDER BY sequence LIMIT 1) WHERE base_revision = ?2)",
        rusqlite::params![name, wanted],
        "read lineage start",
    )?;
    if resulted || started {
        return Ok(true);
    }
    // The chain of forks is at most as long as the forks themselves; a longer walk is a loop.
    if depth > forks.len() {
        return Ok(false);
    }
    let unchanged = flag(
        project,
        project_path,
        "SELECT NOT EXISTS(SELECT 1 FROM operations WHERE lineage_id = ?1)",
        rusqlite::params![name],
        "read lineage operations",
    )?;
    for fork in forks.iter().filter(|fork| fork.revision == revision) {
        if fork.lineage == lineage
            && revision_held(
                project,
                project_path,
                own,
                forks,
                fork.parent,
                revision,
                depth + 1,
            )?
        {
            return Ok(true);
        }
        if unchanged && fork.parent == lineage {
            return Ok(true);
        }
    }
    Ok(false)
}
