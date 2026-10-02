//! Agent runs: validated starts, lifecycle transitions, operation links and derived observation.
//!
//! Nothing here is a project command. A run never appears in the semantic operation log, never
//! changes the plan revision, and cannot submit, verify, release or reassign the task it executes;
//! it only records what was observed about an executor. Persistence lives in `dpm-store`.

use chrono::{DateTime, Utc};
use dpm_model::{
    ActorId, LineageId, OperationId, RunEventId, RunId, RunState, ValidationError, WorkItemId,
    WorkStatus,
};
use thiserror::Error;

mod link;
mod projection;
mod source;
mod start;
mod transition;

pub use link::{OperationFacts, check_link};
pub use projection::{STALE_AFTER, project};
pub use start::observe_start;
pub use transition::{authorize, check_transition};

/// Why a run request was refused.
#[derive(Debug, Error)]
pub enum RunError {
    /// The request is malformed.
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// The task is not in the plan.
    #[error("work item {0} does not exist")]
    MissingWork(WorkItemId),
    /// Only tasks execute; containers and milestones do not.
    #[error("work item {0} is not a task and cannot be executed")]
    NotATask(WorkItemId),
    /// A run starts on claimed or started work, so its executor is already the task's owner.
    #[error("work item {work} is {status:?}; a run starts only on claimed or started work")]
    NotExecuting {
        /// The task.
        work: WorkItemId,
        /// Its lifecycle.
        status: WorkStatus,
    },
    /// The run's executor is not the task's owner.
    #[error("work item {work} is owned by {}, not {executor}", owner.as_ref().map_or_else(|| "no one".to_string(), ToString::to_string))]
    NotOwner {
        /// The task.
        work: WorkItemId,
        /// The executor the run names.
        executor: ActorId,
        /// The task's owner.
        owner: Option<ActorId>,
    },
    /// An agent may record only its own runs; a human or service may record any.
    #[error("actor {actor} may not {action} run {run} executed by {executor}")]
    ActorNotAllowed {
        /// The principal making the request.
        actor: ActorId,
        /// The refused action.
        action: &'static str,
        /// The run.
        run: RunId,
        /// The run's executor.
        executor: ActorId,
    },
    /// Only a service may label a run managed: nothing else can observe the executor.
    #[error("run {0} cannot be managed: only a service actor can record a managed run")]
    ManagedNeedsService(RunId),
    /// A source reference names an asset or artifact the plan does not support.
    #[error("run source is not supported by the plan: {0}")]
    UnsupportedSource(String),
    /// The lifecycle does not allow this transition.
    #[error("run {run} cannot move from {from} to {to}")]
    InvalidTransition {
        /// The run.
        run: RunId,
        /// Its current state.
        from: RunState,
        /// The requested state.
        to: RunState,
    },
    /// The run already ended; a terminal state is final.
    #[error("run {run} already ended as {state}; start a new run to try again")]
    Terminal {
        /// The run.
        run: RunId,
        /// Its terminal state.
        state: RunState,
    },
    /// The operation cannot be attributed to the run.
    #[error("operation {operation} cannot be linked to run {run}: {reason}")]
    LinkRefused {
        /// The run.
        run: RunId,
        /// The operation.
        operation: OperationId,
        /// Why the attribution would be wrong; kept as the error's source.
        #[source]
        reason: LinkRefusal,
    },
}

/// Why an operation cannot be attributed to a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum LinkRefusal {
    /// The operation was committed to another workspace.
    #[error("it belongs to another workspace")]
    OtherWorkspace,
    /// The operation was committed under a lineage other than the one the run observed.
    #[error("it was committed under another lineage than the run observed")]
    OtherLineage,
    /// The operation does not act on the run's task.
    #[error("it does not act on the run's task")]
    OtherWork,
    /// The operation was made by someone other than the run's executor.
    #[error("it was made by someone other than the run's executor")]
    OtherActor,
    /// The operation was committed before the run started.
    #[error("it was committed before the run started")]
    BeforeStart,
    /// The operation was committed after the run ended.
    #[error("it was committed after the run ended")]
    AfterEnd,
}

/// The transition identity of a run's start, which is the run's own identity.
#[must_use]
pub fn start_event(run: RunId) -> RunEventId {
    RunEventId(run.0)
}

/// The instant a receipt at `receipt` stops keeping an unfinished run fresh.
fn stale_after(receipt: DateTime<Utc>) -> Option<DateTime<Utc>> {
    receipt.checked_add_signed(STALE_AFTER)
}

/// Whether `current` is the history a run recorded its contract under.
fn same_history(recorded: LineageId, current: Option<LineageId>) -> bool {
    current == Some(recorded)
}

#[cfg(test)]
mod tests;
