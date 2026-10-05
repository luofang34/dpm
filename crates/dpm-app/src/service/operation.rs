//! Idempotent commits: a client-supplied operation identity is also the idempotency key.
//!
//! An identity is looked up before any precondition, because a resend usually carries a base
//! revision the first attempt has since moved past, and again inside the store's write
//! transaction, where a concurrent attempt with the same identity may have committed first. A
//! resend matching the recorded actor and content returns the recorded operation unchanged; any
//! other content under that identity is refused, so one identity never names two changes.

use super::{Application, Backing, Intent, Preconditions, WorkspaceRevision};
use crate::AppError;
use chrono::Utc;
use dpm_engine::{Command, Operation};
use dpm_model::{ActorId, LineageId, OperationId, Plan};
use dpm_store::{HistoryEntry, RecordedOperation, SqliteStore, StoreError};

impl Application {
    pub(super) fn commit_blocking(
        &mut self,
        preconditions: Preconditions,
        actor: ActorId,
        intent: Intent,
    ) -> Result<RecordedOperation, AppError> {
        self.ensure_writable()?;
        let id = operation_id(preconditions.operation_id)?;
        let Backing::Database(store) = &mut self.backing else {
            return Err(AppError::ReadOnlyProject);
        };
        let committed = commit_to_blocking(store, preconditions, &actor, &intent, id)?;
        if let Committed::New(operation) = &committed {
            self.watchers.notify(WorkspaceRevision {
                revision: operation.operation.resulting_revision,
                lineage_id: Some(operation.lineage_id),
            });
        }
        Ok(match committed {
            Committed::New(operation) | Committed::Recorded(operation) => operation,
        })
    }

    /// Answer a failed command build with `duplicate_operation` and the recorded operation when
    /// the supplied identity is already on the log, since the failure may only reflect state that
    /// identity's first attempt changed; any other outcome passes through unchanged.
    pub(super) fn recorded_instead_blocking<T, E: From<AppError>>(
        &self,
        built: Result<T, E>,
        operation_id: Option<OperationId>,
    ) -> Result<T, E> {
        let (Err(_), Some(id), Backing::Database(store)) = (&built, operation_id, &self.backing)
        else {
            return built;
        };
        match store
            .recorded_operation_blocking(id)
            .map_err(AppError::from)?
        {
            Some(recorded) => Err(AppError::DuplicateOperation {
                recorded: Box::new(recorded.operation),
            }
            .into()),
            None => built,
        }
    }
}

/// Whether a mutation wrote a new operation or was answered with one already recorded.
enum Committed {
    New(RecordedOperation),
    Recorded(RecordedOperation),
}

fn commit_to_blocking(
    store: &mut SqliteStore,
    preconditions: Preconditions,
    actor: &ActorId,
    intent: &Intent,
    id: OperationId,
) -> Result<Committed, AppError> {
    if let Some(recorded) = store.recorded_operation_blocking(id)? {
        return answer_resend(store, recorded, actor, intent).map(Committed::Recorded);
    }
    match attempt_blocking(store, preconditions, actor, intent, id) {
        // A concurrent attempt with the same identity may commit between the lookup above and
        // the checks here; its conflict or refusal must not hide that the change is recorded.
        Err(error) if preconditions.operation_id.is_some() => {
            match store.recorded_operation_blocking(id)? {
                Some(recorded) => {
                    answer_resend(store, recorded, actor, intent).map(Committed::Recorded)
                }
                None => Err(error),
            }
        }
        outcome => outcome,
    }
}

fn attempt_blocking(
    store: &mut SqliteStore,
    preconditions: Preconditions,
    actor: &ActorId,
    intent: &Intent,
    id: OperationId,
) -> Result<Committed, AppError> {
    check_lineage(store, preconditions.base_lineage)?;
    let mut plan = store.load_blocking()?.ok_or(AppError::NotInitialized)?;
    if preconditions.base_revision != plan.revision {
        return Err(AppError::Conflict {
            expected: preconditions.base_revision,
            actual: plan.revision,
        });
    }
    let operation = apply(&mut plan, actor.clone(), intent, id)?;
    match store.persist_blocking(&plan, &operation, preconditions.base_lineage) {
        Ok(entry) => Ok(Committed::New(entry.operation)),
        Err(StoreError::DuplicateOperation { recorded }) => {
            answer_resend(store, *recorded, actor, intent).map(Committed::Recorded)
        }
        Err(error) => Err(error.into()),
    }
}

/// A supplied identity must be a version 7 UUID; without one the application mints it.
fn operation_id(supplied: Option<OperationId>) -> Result<OperationId, AppError> {
    match supplied {
        Some(id) if id.is_time_ordered() => Ok(id),
        Some(id) => Err(AppError::InvalidRequest(format!(
            "operation id {id} is not a version 7 UUID"
        ))),
        None => Ok(OperationId::new()),
    }
}

/// Refuse an archive or another lineage before the engine runs; the store re-checks under its lock.
fn check_lineage(store: &SqliteStore, expected: Option<LineageId>) -> Result<(), AppError> {
    let found = store.lineage_blocking()?.ok_or(AppError::NotInitialized)?;
    found
        .check_writable(expected, store.path())
        .map_err(|error| AppError::Store(error.into()))
}

fn apply(
    plan: &mut Plan,
    actor: ActorId,
    intent: &Intent,
    id: OperationId,
) -> Result<Operation, AppError> {
    let now = Utc::now();
    Ok(match intent {
        Intent::Command(command) => {
            dpm_engine::apply_command(plan, actor, command.clone(), now, id)?
        }
        Intent::PlanChange { proposed, reason } => {
            dpm_engine::apply_plan_change(plan, actor, proposed, reason.clone(), now, id)?
        }
    })
}

/// Return the recorded operation for a matching resend, and refuse any other content.
///
/// A plan change records only its difference, so the resent proposal is diffed again against the
/// state the recorded change was applied to.
fn answer_resend(
    store: &SqliteStore,
    recorded: HistoryEntry,
    actor: &ActorId,
    intent: &Intent,
) -> Result<RecordedOperation, AppError> {
    let operation = &recorded.operation.operation;
    let same_content = match intent {
        Intent::Command(command) => same(command, &operation.command)?,
        Intent::PlanChange { proposed, reason } => {
            let base = store.plan_before_blocking(recorded.sequence)?;
            match dpm_engine::plan_change(&base, proposed, reason.clone()) {
                Ok(command) => same(&command, &operation.command)?,
                Err(_) => false,
            }
        }
    };
    if same_content && operation.actor == *actor {
        Ok(recorded.operation)
    } else {
        Err(AppError::DuplicateOperation {
            recorded: Box::new(recorded.operation),
        })
    }
}

/// Commands compare by their serialized form, the form the log records, with numbers compared by value: a
/// resent reviewed change may carry `2` where the log recorded `2.0`.
fn same(left: &Command, right: &Command) -> Result<bool, AppError> {
    Ok(dpm_engine::json_equivalent(
        &serde_json::to_value(left)?,
        &serde_json::to_value(right)?,
    ))
}

#[cfg(test)]
mod tests;
