//! Opening the run store beside the project store, lazily and without ever creating it on a read.

use super::super::{Application, Backing};
use crate::AppError;
use dpm_engine::OperationFacts;
use dpm_model::{
    ActivityPage, LifecyclePage, LineageId, OperationId, RunFeedHeads, RunId, RunSnapshot,
    WorkItemId,
};
use dpm_store::{RUN_ACTIVITY_LIMIT, RunStore, RunStoreError};
use std::{
    cell::{Cell, RefCell},
    path::Path,
};

/// The run store of one [`Application`], opened the first time it is needed.
///
/// An `Application` is `Send` but not `Sync`, so interior mutability is enough: reads open an
/// existing store without creating one, and the first write creates it.
pub(in crate::service) struct RunSlot {
    store: RefCell<Option<RunStore>>,
    limit: Cell<u64>,
}

impl Default for RunSlot {
    fn default() -> Self {
        Self {
            store: RefCell::new(None),
            limit: Cell::new(RUN_ACTIVITY_LIMIT),
        }
    }
}

impl Application {
    /// Keep at most `max_activity` activity records; older ones are pruned and reported as a gap
    /// to any cursor that outran them. Lifecycle facts are never pruned.
    pub fn set_run_retention(&mut self, max_activity: u64) {
        self.runs.limit.set(max_activity);
        if let Some(store) = self.runs.store.borrow_mut().as_mut() {
            store.set_activity_limit(max_activity);
        }
    }

    /// Open the run store if it exists, or create it when `create` names the lineage to bind it to.
    /// Returns whether a run store is now open.
    fn open_runs_blocking(&self, create: Option<LineageId>) -> Result<bool, AppError> {
        let Backing::Database(project) = &self.backing else {
            return Ok(false);
        };
        let mut slot = self.runs.store.borrow_mut();
        if slot.is_some() {
            return Ok(true);
        }
        let workspace = self.plan_blocking()?.workspace.id;
        let sidecar = RunStore::sidecar_path(project.path());
        let in_memory = project.path() == Path::new(":memory:");
        let opened = match (create, in_memory) {
            (Some(lineage), true) => Some(RunStore::in_memory_blocking(workspace, lineage)?),
            (None, true) => None,
            (Some(lineage), false) => Some(RunStore::open_or_create_blocking(
                &sidecar, workspace, lineage,
            )?),
            (None, false) => RunStore::open_existing_blocking(&sidecar, workspace)?,
        };
        let Some(mut store) = opened else {
            return Ok(false);
        };
        store.set_activity_limit(self.runs.limit.get());
        *slot = Some(store);
        Ok(true)
    }

    /// Read from the run store, or `None` when no run store is found beside the project store.
    fn with_runs_blocking<T>(
        &self,
        read: impl FnOnce(&RunStore) -> Result<T, RunStoreError>,
    ) -> Result<Option<T>, AppError> {
        if !self.open_runs_blocking(None)? {
            return Ok(None);
        }
        let slot = self.runs.store.borrow();
        let Some(store) = slot.as_ref() else {
            return Ok(None);
        };
        Ok(Some(read(store)?))
    }

    /// Write to the run store, creating it bound to the current lineage if needed.
    ///
    /// The write is refused on a preview, on a project store that is a backup archive, and for a
    /// lineage other than `base`, if given. Those checks come first: nothing is opened, created
    /// or replayed for a store that may not be written, so a backup is never treated as a live
    /// workspace just because it has no run store beside it.
    pub(super) fn with_runs_mut_blocking<T>(
        &self,
        base: Option<LineageId>,
        write: impl FnOnce(&mut RunStore, LineageId) -> Result<T, RunStoreError>,
    ) -> Result<T, AppError> {
        self.ensure_writable()?;
        let Backing::Database(project) = &self.backing else {
            return Err(AppError::ReadOnlyProject);
        };
        let found = project
            .lineage_blocking()?
            .ok_or(AppError::NotInitialized)?;
        found
            .check_writable(base, project.path())
            .map_err(|error| AppError::Store(error.into()))?;
        let current = found.lineage_id;
        self.open_runs_blocking(Some(current))?;
        let mut slot = self.runs.store.borrow_mut();
        let store = slot.as_mut().ok_or(AppError::NotInitialized)?;
        Ok(write(store, current)?)
    }

    pub(super) fn run_snapshot_blocking(&self, id: RunId) -> Result<Option<RunSnapshot>, AppError> {
        Ok(self
            .with_runs_blocking(|store| store.snapshot_blocking(id))?
            .flatten())
    }

    pub(super) fn run_snapshots_blocking(
        &self,
        work: Option<WorkItemId>,
        limit: u16,
    ) -> Result<Vec<RunSnapshot>, AppError> {
        Ok(self
            .with_runs_blocking(|store| store.snapshots_blocking(work, limit))?
            .unwrap_or_default())
    }

    pub(in crate::service) fn run_lifecycle_page_blocking(
        &self,
        after: u64,
        limit: u16,
        run: Option<RunId>,
    ) -> Result<LifecyclePage, AppError> {
        Ok(self
            .with_runs_blocking(|store| store.lifecycle_page_blocking(after, limit, run))?
            .unwrap_or(LifecyclePage {
                entries: Vec::new(),
                next_after_sequence: after,
                head_sequence: 0,
            }))
    }

    pub(in crate::service) fn run_activity_page_blocking(
        &self,
        after: u64,
        limit: u16,
        run: Option<RunId>,
    ) -> Result<ActivityPage, AppError> {
        Ok(self
            .with_runs_blocking(|store| store.activity_page_blocking(after, limit, run))?
            .unwrap_or(ActivityPage {
                entries: Vec::new(),
                next_after_sequence: after,
                head_sequence: 0,
                gap: None,
            }))
    }

    /// Where the run feeds and the link count stand, in one read transaction; all zero and without
    /// an epoch while no run store exists.
    pub(in crate::service) fn run_heads_blocking(&self) -> Result<RunFeedHeads, AppError> {
        Ok(self
            .with_runs_blocking(|store| store.heads_blocking())?
            .unwrap_or_default())
    }

    /// What the project store recorded about a committed operation, for linking it to a run.
    pub(super) fn recorded_operation_facts_blocking(
        &self,
        id: OperationId,
    ) -> Result<OperationFacts, AppError> {
        let Backing::Database(store) = &self.backing else {
            return Err(AppError::ReadOnlyProject);
        };
        let recorded = store
            .recorded_operation_blocking(id)?
            .ok_or(AppError::UnknownOperation(id))?
            .operation;
        Ok(OperationFacts::of(
            &recorded.operation,
            recorded.workspace_id,
            recorded.lineage_id,
        ))
    }
}
