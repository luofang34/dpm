//! Atomic, idempotent run writes. None of them touches the project store.
//!
//! Each write is one transaction under the run store's own write lock, so a heartbeat can never
//! hold up a project command. A resent identity is looked up inside the transaction: identical
//! content returns what was recorded, different content is refused, and nothing is written twice.

use super::{
    RunStore, Written,
    codec::{optional_time_text, time_text},
    read::{event_by_id, last_event, link_by_operation, load_run},
    read_binding, schema,
};
use crate::{
    RunStoreError, StoreError, StoreLineage, StoredRecord, error::database_error, schema::Layout,
};
use chrono::{DateTime, Utc};
use dpm_engine::{
    OperationFacts, RunError, authorize_run, check_link, check_run_transition, run_start_event,
};
use dpm_model::{
    ActivityEntry, ActivityInput, ActorId, LifecycleEntry, LifecycleEvent, LineageId,
    MAX_ACTIVITY_BATCH, RunId, RunLink, RunRecord, RunState, RunTransition, ValidationError,
};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use std::{collections::BTreeSet, path::Path};

pub(super) fn to_json<T: serde::Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(StoreError::from)
}

/// Begin a write transaction after re-checking the layout and the binding under the lock: an
/// archive is never written, and neither is a store bound to another history than the caller's.
fn begin<'a>(
    connection: &'a mut Connection,
    path: &Path,
    expected: LineageId,
) -> Result<Transaction<'a>, StoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database_error(path, "begin run write"))?;
    if schema::check_blocking(&transaction, path)? == Layout::Empty {
        return Err(StoreError::NotInitialized(path.to_path_buf()));
    }
    let binding = read_binding(&transaction, path)?;
    StoreLineage {
        lineage_id: binding.lineage_id,
        archived: binding.archived,
    }
    .check_writable(Some(expected), path)?;
    Ok(transaction)
}

/// A run the store may take new facts for: it exists and was recorded under the lineage the store
/// continues. A run from before a restore is a historical observation and is never written to,
/// whether the request is new or a retry.
fn current_run(
    transaction: &Transaction<'_>,
    path: &Path,
    id: RunId,
    expected: LineageId,
) -> Result<RunRecord, RunStoreError> {
    let record = load_run(transaction, path, id)?.ok_or(RunStoreError::UnknownRun(id))?;
    if record.contract.lineage_id != expected {
        return Err(RunStoreError::ForeignRun {
            run: id,
            recorded: record.contract.lineage_id,
            current: expected,
        });
    }
    Ok(record)
}

fn writable_run(
    transaction: &Transaction<'_>,
    path: &Path,
    id: RunId,
    expected: LineageId,
    writer: &ActorId,
    action: &'static str,
) -> Result<RunRecord, RunStoreError> {
    let record = current_run(transaction, path, id, expected)?;
    authorize_run(&record, writer, action)?;
    Ok(record)
}

fn commit(transaction: Transaction<'_>, path: &Path) -> Result<(), StoreError> {
    transaction
        .commit()
        .map_err(database_error(path, "commit run write"))
}

fn insert_event(
    transaction: &Transaction<'_>,
    path: &Path,
    event: &LifecycleEvent,
) -> Result<u64, StoreError> {
    transaction
        .execute(
            "INSERT INTO run_lifecycle(event_id, run_id, state, detail, observed_at, \
             recorded_by_json, recorded_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event.id.to_string(),
                event.run.to_string(),
                event.state.word(),
                event.detail,
                optional_time_text(event.observed_at),
                to_json(&event.recorded_by)?,
                time_text(event.recorded_at),
            ],
        )
        .map_err(database_error(path, "append lifecycle fact"))?;
    u64::try_from(transaction.last_insert_rowid()).map_err(|_| corrupt_sequence(path))
}

pub(super) fn corrupt_sequence(path: &Path) -> StoreError {
    StoreError::CorruptColumn {
        path: path.to_path_buf(),
        record: StoredRecord::Run,
        field: "sequence",
        source: rusqlite::Error::IntegralValueOutOfRange(0, 0),
    }
}

fn invalid_batch(reason: String) -> RunStoreError {
    RunStoreError::Run(RunError::Validation(ValidationError::Invalid {
        entity: "run activity",
        id: "batch".into(),
        reason,
    }))
}

impl RunStore {
    /// Record a run's start as its first lifecycle fact, in the history `expected`.
    ///
    /// The run identity is the idempotency key: a resent start that matches the recorded one
    /// returns it unchanged, and any other content under that identity is refused.
    pub fn start_blocking(
        &mut self,
        record: &RunRecord,
        expected: LineageId,
    ) -> Result<Written<LifecycleEntry>, RunStoreError> {
        let path = self.path.clone();
        let transaction = begin(&mut self.connection, &path, expected)?;
        if load_run(&transaction, &path, record.id)?.is_some() {
            let recorded = current_run(&transaction, &path, record.id, expected)?;
            if recorded.request() != record.request() || recorded.recorded_by != record.recorded_by
            {
                return Err(RunStoreError::DuplicateRun {
                    recorded: Box::new(recorded),
                });
            }
            let first = event_by_id(&transaction, &path, run_start_event(record.id))?
                .ok_or(RunStoreError::UnknownRun(record.id))?;
            return Ok(Written {
                value: first,
                replayed: true,
            });
        }
        if let Some(parent) = record.parent
            && load_run(&transaction, &path, parent)?.is_none()
        {
            return Err(RunStoreError::UnknownParent(parent));
        }
        transaction
            .execute(
                "INSERT INTO runs(run_id, work_id, record_json) VALUES(?1, ?2, ?3)",
                params![
                    record.id.to_string(),
                    record.work.to_string(),
                    to_json(record)?
                ],
            )
            .map_err(database_error(&path, "record run"))?;
        let event = LifecycleEvent {
            id: run_start_event(record.id),
            run: record.id,
            state: RunState::Working,
            detail: None,
            observed_at: record.observed_started_at,
            recorded_by: record.recorded_by.clone(),
            recorded_at: record.started_at,
        };
        let sequence = insert_event(&transaction, &path, &event)?;
        commit(transaction, &path)?;
        Ok(Written {
            value: LifecycleEntry { sequence, event },
            replayed: false,
        })
    }

    /// Record a lifecycle transition as `writer`, at DPM's own time `now`.
    ///
    /// The transition identity is the idempotency key. The state machine is checked inside the
    /// write transaction against the run's latest recorded state, so two racing reporters cannot
    /// both end a run.
    pub fn transition_blocking(
        &mut self,
        transition: &RunTransition,
        writer: &ActorId,
        now: DateTime<Utc>,
        expected: LineageId,
    ) -> Result<Written<LifecycleEntry>, RunStoreError> {
        transition.validate().map_err(RunError::from)?;
        let path = self.path.clone();
        let transaction = begin(&mut self.connection, &path, expected)?;
        if let Some(recorded) = event_by_id(&transaction, &path, transition.id)? {
            current_run(&transaction, &path, recorded.event.run, expected)?;
            let event = &recorded.event;
            let same = event.run == transition.run
                && event.state == transition.to
                && event.detail == transition.detail
                && event.observed_at == transition.observed_at
                && event.recorded_by == *writer;
            return if same {
                Ok(Written {
                    value: recorded,
                    replayed: true,
                })
            } else {
                Err(RunStoreError::DuplicateEvent {
                    recorded: Box::new(recorded),
                })
            };
        }
        let record = writable_run(
            &transaction,
            &path,
            transition.run,
            expected,
            writer,
            "report on",
        )?;
        let current = last_event(&transaction, &path, record.id)?
            .ok_or(RunStoreError::UnknownRun(record.id))?;
        check_run_transition(record.id, current.event.state, transition.to)?;
        let event = LifecycleEvent {
            id: transition.id,
            run: record.id,
            state: transition.to,
            detail: transition.detail.clone(),
            observed_at: transition.observed_at,
            recorded_by: writer.clone(),
            recorded_at: now,
        };
        let sequence = insert_event(&transaction, &path, &event)?;
        commit(transaction, &path)?;
        Ok(Written {
            value: LifecycleEntry { sequence, event },
            replayed: false,
        })
    }

    /// Record a batch of activity as `writer`, at DPM's own time `now`, and apply retention.
    ///
    /// All of the batch is recorded or none of it. A record is identified by its run and
    /// `source_sequence`. One still held with the same content, including the whole of its text,
    /// is a duplicate delivery and is answered with the recorded record; with other content it is
    /// refused. A sequence at or below the run's high-water mark that is no longer held is refused
    /// as expired, so a retry that outlived retention can never become a new receipt. Activity
    /// changes no lifecycle state, whatever it reports.
    pub fn append_activity_blocking(
        &mut self,
        inputs: &[ActivityInput],
        writer: &ActorId,
        now: DateTime<Utc>,
        expected: LineageId,
    ) -> Result<Vec<Written<ActivityEntry>>, RunStoreError> {
        if inputs.is_empty() || inputs.len() > MAX_ACTIVITY_BATCH {
            return Err(invalid_batch(format!(
                "a batch carries 1..={MAX_ACTIVITY_BATCH} records, not {}",
                inputs.len()
            )));
        }
        for input in inputs {
            input.validate().map_err(RunError::from)?;
        }
        let path = self.path.clone();
        let limit = self.activity_limit;
        let transaction = begin(&mut self.connection, &path, expected)?;
        let mut authorized = BTreeSet::new();
        let mut written = Vec::with_capacity(inputs.len());
        for input in inputs {
            if !authorized.contains(&input.run) {
                writable_run(
                    &transaction,
                    &path,
                    input.run,
                    expected,
                    writer,
                    "record activity for",
                )?;
                authorized.insert(input.run);
            }
            written.push(super::activity::append_one(
                &transaction,
                &path,
                input,
                writer,
                now,
            )?);
        }
        super::activity::prune(&transaction, &path, limit)?;
        commit(transaction, &path)?;
        Ok(written)
    }

    /// Record that a committed project operation was performed by a run, as `writer`.
    ///
    /// `operation` carries what the project store recorded about the operation; the engine checks
    /// it inside the write transaction against the run, so a link can never attribute someone
    /// else's operation, another task's, or one outside the run's lifetime. An operation links to
    /// at most one run; repeating the same link answers with the recorded one.
    pub fn link_blocking(
        &mut self,
        run: RunId,
        operation: &OperationFacts,
        writer: &ActorId,
        now: DateTime<Utc>,
        expected: LineageId,
    ) -> Result<Written<RunLink>, RunStoreError> {
        let path = self.path.clone();
        let transaction = begin(&mut self.connection, &path, expected)?;
        let record = writable_run(
            &transaction,
            &path,
            run,
            expected,
            writer,
            "link operations to",
        )?;
        if let Some(linked) = link_by_operation(&transaction, &path, operation.id)? {
            return if linked.run == run {
                Ok(Written {
                    value: linked,
                    replayed: true,
                })
            } else {
                Err(RunStoreError::LinkConflict {
                    operation: operation.id,
                    linked: linked.run,
                })
            };
        }
        let current =
            last_event(&transaction, &path, run)?.ok_or(RunStoreError::UnknownRun(run))?;
        let ended = current
            .event
            .state
            .is_terminal()
            .then_some(current.event.recorded_at);
        check_link(&record, ended, operation)?;
        let link = RunLink {
            run,
            operation: operation.id,
            work: record.work,
            linked_by: writer.clone(),
            linked_at: now,
        };
        transaction
            .execute(
                "INSERT INTO run_links(operation_id, run_id, work_id, linked_by_json, linked_at) \
                 VALUES(?1, ?2, ?3, ?4, ?5)",
                params![
                    link.operation.to_string(),
                    link.run.to_string(),
                    link.work.to_string(),
                    to_json(&link.linked_by)?,
                    time_text(link.linked_at),
                ],
            )
            .map_err(database_error(&path, "record run link"))?;
        commit(transaction, &path)?;
        Ok(Written {
            value: link,
            replayed: false,
        })
    }
}
