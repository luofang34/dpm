//! Idempotent commits: a client-supplied operation identity is also the idempotency key.
//!
//! An identity is looked up before any precondition, because a resend usually carries a base
//! revision the first attempt has since moved past, and again inside the store's write
//! transaction, where a concurrent attempt with the same identity may have committed first. A
//! resend matching the recorded actor and content returns the recorded operation unchanged; any
//! other content under that identity is refused, so one identity never names two changes.

use super::{Application, Backing};
use crate::AppError;
use chrono::Utc;
use dpm_engine::{Command, Operation};
use dpm_model::{ActorId, LineageId, OperationId, Plan};
use dpm_store::{HistoryEntry, RecordedOperation, SqliteStore, StoreError};

/// Client identity and preconditions shared by every mutation.
#[derive(Debug, Clone, Copy)]
pub(super) struct Preconditions {
    pub(super) base_revision: u64,
    pub(super) base_lineage: Option<LineageId>,
    pub(super) operation_id: Option<OperationId>,
}

/// What the client asked to change, kept in the form a resend is compared in.
pub(super) enum Intent {
    Command(Command),
    PlanChange { proposed: Box<Plan>, reason: String },
}

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
        if let Some(recorded) = store.recorded_operation_blocking(id)? {
            return answer_resend(store, recorded, &actor, &intent);
        }
        check_lineage(store, preconditions.base_lineage)?;
        let mut plan = store.load_blocking()?.ok_or(AppError::NotInitialized)?;
        if preconditions.base_revision != plan.revision {
            return Err(AppError::Conflict {
                expected: preconditions.base_revision,
                actual: plan.revision,
            });
        }
        let operation = apply(&mut plan, actor.clone(), &intent, id)?;
        match store.persist_blocking(&plan, &operation, preconditions.base_lineage) {
            Ok(entry) => Ok(entry.operation),
            Err(StoreError::DuplicateOperation { recorded }) => {
                answer_resend(store, *recorded, &actor, &intent)
            }
            Err(error) => Err(error.into()),
        }
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

/// Commands compare by their serialized form, the form the log records.
fn same(left: &Command, right: &Command) -> Result<bool, AppError> {
    Ok(serde_json::to_value(left)? == serde_json::to_value(right)?)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
