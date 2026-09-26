//! Provisional start releases and derived basis invalidation, evaluated inside the shared gate
//! evaluator so every view and command reads the same result.

use super::UnmetGate;
use dpm_model::{BasisState, DependencyPolicy, Endpoint, Plan, Release, WorkItem, basis_status};

/// One gate per enforced edge whose effective basis names a rejected predecessor attempt.
pub(super) fn basis_gates(plan: &Plan, work: &WorkItem) -> Vec<UnmetGate> {
    basis_status(plan, work)
        .into_iter()
        .filter(dpm_model::BasisStatus::gates)
        .map(|status| UnmetGate::BasisInvalidated {
            dependency: status.dependency,
            predecessor: status.predecessor,
            key: status.predecessor_key,
            attempt: status.basis.attempt,
            state: status.state,
            current_attempt: status.current_attempt,
        })
        .collect()
}

impl super::ProvisionalRelease {
    /// Human-readable reliance on an unverified attempt, shared by `explain` and `next`.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "provisional: {} attempt #{} is only submitted; a start relies on it, and verification still requires its verified finish",
            self.key, self.attempt
        )
    }
}

pub(super) fn describe_start(
    key: &str,
    release: Release,
    attempt: Option<u32>,
    edge: &str,
    policy: DependencyPolicy,
) -> String {
    match (release, attempt) {
        (Release::Elapsing { event_at, opens_at }, Some(n)) => format!(
            "{key} attempt #{n} submitted at {event_at}; provisional lag elapses at {opens_at} ({edge})"
        ),
        (Release::LagOutOfRange { event_at }, Some(n)) => format!(
            "{key} attempt #{n} submitted at {event_at}, but the lag exceeds the supported time range ({edge})"
        ),
        (Release::AwaitingEvent, _) => format!(
            "awaits a pending submission attempt or the verified finish of {key} ({edge}, provisional start)"
        ),
        _ => super::transition::describe_release(key, Endpoint::Finish, release, edge, policy),
    }
}

pub(super) fn describe_invalidated(
    key: &str,
    attempt: u32,
    state: &BasisState,
    current: Option<u32>,
) -> String {
    let rejection = match state {
        BasisState::Invalidated {
            rejected_by,
            rejected_at,
            reason,
        } => format!(" by {rejected_by} at {rejected_at}: {reason}"),
        BasisState::Pending | BasisState::Verified => String::new(),
    };
    let remedy = match current {
        Some(n) => format!(
            "an independent human or service may revalidate this work against {key} attempt #{n}"
        ),
        None => format!(
            "{key} has no pending or verified attempt yet; revalidation waits for its resubmission"
        ),
    };
    format!(
        "execution relies on {key} attempt #{attempt}, which was rejected{rejection}; a later submission or verification never validates it silently: {remedy}"
    )
}
