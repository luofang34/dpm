//! Who may report on a run, and which lifecycle moves are legal.

use super::RunError;
use dpm_model::{ActorId, ActorKind, RunId, RunRecord, RunState};

/// Refuse an agent that is not the run's executor; humans and services may record any run.
///
/// Runs observe an executor, so an agent must not be able to complete or fail another agent's run,
/// while a service host or an operator can speak for a run it watches or recovers.
pub fn authorize(
    record: &RunRecord,
    writer: &ActorId,
    action: &'static str,
) -> Result<(), RunError> {
    authorize_writer(writer, &record.executor, record.id, action)
}

pub(super) fn authorize_writer(
    writer: &ActorId,
    executor: &ActorId,
    run: RunId,
    action: &'static str,
) -> Result<(), RunError> {
    if writer == executor || writer.kind != ActorKind::Agent {
        return Ok(());
    }
    Err(RunError::ActorNotAllowed {
        actor: writer.clone(),
        action,
        run,
        executor: executor.clone(),
    })
}

/// Check a move from the run's current state.
///
/// Working and waiting alternate, and either may end in any terminal state. A terminal state is
/// final, and repeating the current state is not a transition.
pub fn check_transition(run: RunId, from: RunState, to: RunState) -> Result<(), RunError> {
    if from.is_terminal() {
        return Err(RunError::Terminal { run, state: from });
    }
    if from == to {
        return Err(RunError::InvalidTransition { run, from, to });
    }
    Ok(())
}
