use crate::{
    ActorId, Dependency, DependencyId, EventTime, Plan, Release, StartBasis, Timeline, WorkItem,
    WorkItemId,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One submission of a task, identified by its 1-based ordinal within that task.
///
/// The ordinal is deterministic, so every reload and replica of the same history agrees on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmissionAttempt {
    /// Position of this attempt in the task's submission history, starting at 1.
    pub number: u32,
    /// Time recorded by the submit command.
    pub submitted_at: DateTime<Utc>,
    /// Review result; only the latest attempt may still be pending.
    pub outcome: AttemptOutcome,
}

/// Independent review result of one submission attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttemptOutcome {
    /// Awaiting independent review.
    Pending,
    /// Returned for rework; the attempt stays in history and never becomes valid again.
    Rejected {
        /// Independent reviewer.
        actor: ActorId,
        /// Time recorded by the reject command.
        at: DateTime<Utc>,
        /// Concrete unmet acceptance or evidence requirement.
        reason: String,
    },
    /// Accepted by an independent verifier.
    Verified {
        /// Independent verifier.
        actor: ActorId,
        /// Time recorded by the verify command.
        at: DateTime<Utc>,
    },
}

/// Immutable reference to the predecessor attempt a successor's execution relies on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyBasis {
    /// Provisional edge whose start gate the attempt released.
    pub dependency: DependencyId,
    /// Predecessor that made the attempt.
    pub predecessor: WorkItemId,
    /// Ordinal of the relied-on predecessor attempt.
    pub attempt: u32,
    /// Time recorded by the operation that captured this basis.
    pub recorded_at: DateTime<Utc>,
    /// Operation that captured this basis.
    pub source: BasisSource,
}

/// Operation that recorded a dependency basis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BasisSource {
    /// The successor's owner started on the attempt through the provisional start gate.
    Start,
    /// An independent human or service reviewed the successor against a newer attempt.
    Revalidation {
        /// Accountable reviewer.
        actor: ActorId,
        /// Why the successor's work remains valid on the new attempt.
        reason: String,
    },
}

impl WorkItem {
    /// Attempt with the given ordinal.
    #[must_use]
    pub fn attempt(&self, number: u32) -> Option<&SubmissionAttempt> {
        let index = usize::try_from(number).ok()?.checked_sub(1)?;
        self.attempts.get(index)
    }

    /// Latest attempt unless it was rejected: the pending or verified result a basis may rely on.
    #[must_use]
    pub fn current_attempt(&self) -> Option<&SubmissionAttempt> {
        self.attempts
            .last()
            .filter(|a| !matches!(a.outcome, AttemptOutcome::Rejected { .. }))
    }

    /// Latest basis recorded for each edge, in first-recorded edge order.
    #[must_use]
    pub fn effective_basis(&self) -> Vec<&DependencyBasis> {
        let mut latest: Vec<&DependencyBasis> = Vec::new();
        for entry in &self.basis {
            match latest.iter_mut().find(|b| b.dependency == entry.dependency) {
                Some(slot) => *slot = entry,
                None => latest.push(entry),
            }
        }
        latest
    }
}

/// Derived validity of a recorded basis; never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BasisState {
    /// The relied-on attempt still awaits review.
    Pending,
    /// The relied-on attempt was verified.
    Verified,
    /// The relied-on attempt was rejected; only an explicit revalidation records a new basis.
    Invalidated {
        /// Reviewer who rejected the attempt.
        rejected_by: ActorId,
        /// Time of the rejection.
        rejected_at: DateTime<Utc>,
        /// Rejection reason.
        reason: String,
    },
}

/// A successor's effective basis on one provisional edge, with its derived state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BasisStatus {
    /// Work whose execution relies on the attempt.
    pub successor: WorkItemId,
    /// Human-readable successor key.
    pub successor_key: String,
    /// Provisional edge carrying the basis.
    pub dependency: DependencyId,
    /// Work that made the attempt.
    pub predecessor: WorkItemId,
    /// Human-readable predecessor key.
    pub predecessor_key: String,
    /// The immutable recorded reference.
    pub basis: DependencyBasis,
    /// Derived from the predecessor's attempt record.
    pub state: BasisState,
    /// Predecessor's pending or verified attempt a revalidation may name, if any.
    pub current_attempt: Option<u32>,
    /// Whether the edge gates execution; a waived edge's basis is context only.
    pub enforced: bool,
}

impl BasisStatus {
    /// Whether an enforced basis rests on a rejected attempt and so gates finishing the successor.
    #[must_use]
    pub fn gates(&self) -> bool {
        self.enforced && matches!(self.state, BasisState::Invalidated { .. })
    }
}

/// Effective basis of a work item on each provisional edge, derived from attempt records.
#[must_use]
pub fn basis_status(plan: &Plan, work: &WorkItem) -> Vec<BasisStatus> {
    work.effective_basis()
        .into_iter()
        .filter_map(|basis| status_of(plan, work, basis))
        .collect()
}

/// Effective bases of other work that rely on attempts of this predecessor.
#[must_use]
pub fn basis_dependents(plan: &Plan, predecessor: WorkItemId) -> Vec<BasisStatus> {
    plan.work_items
        .values()
        .flat_map(|work| basis_status(plan, work))
        .filter(|status| status.predecessor == predecessor)
        .collect()
}

fn status_of(plan: &Plan, work: &WorkItem, basis: &DependencyBasis) -> Option<BasisStatus> {
    let predecessor = plan.work_items.get(&basis.predecessor)?;
    let state = match &predecessor.attempt(basis.attempt)?.outcome {
        AttemptOutcome::Pending => BasisState::Pending,
        AttemptOutcome::Verified { .. } => BasisState::Verified,
        AttemptOutcome::Rejected { actor, at, reason } => BasisState::Invalidated {
            rejected_by: actor.clone(),
            rejected_at: *at,
            reason: reason.clone(),
        },
    };
    Some(BasisStatus {
        successor: work.id,
        successor_key: work.key.to_string(),
        dependency: basis.dependency,
        predecessor: predecessor.id,
        predecessor_key: predecessor.key.to_string(),
        basis: basis.clone(),
        state,
        current_attempt: predecessor.current_attempt().map(|a| a.number),
        enforced: plan
            .find_dependency(basis.dependency)
            .is_some_and(|edge| !edge.is_waived()),
    })
}

/// Release of an edge for the successor's start, and the predecessor attempt it was measured from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StartRelease {
    /// Release state at the timeline's clock reading.
    pub release: Release,
    /// Predecessor attempt whose submission time a provisional release is measured from.
    pub attempt: Option<u32>,
    /// Whether that attempt still awaits review, so a start on it relies on an unverified result.
    pub provisional: bool,
}

impl Timeline {
    /// Evaluate an edge for the successor's claim or start.
    ///
    /// A provisional edge releases on the predecessor's current attempt's submission plus positive
    /// lag until its verified finish releases it, so verifying that attempt never pushes an
    /// elapsing start gate later. Every other evaluation, including the successor's verification
    /// and milestone reach, uses [`Timeline::edge`], so a submission never counts as a finish
    /// there. A legacy submission without an attempt record has nothing a basis could reference,
    /// so its edge waits for verification.
    #[must_use]
    pub fn start_edge(&self, plan: &Plan, edge: &Dependency) -> StartRelease {
        let release = self.edge(plan, edge);
        let current = (edge.start_basis == StartBasis::Provisional
            && release.released_at().is_none())
        .then(|| plan.work_items.get(&edge.predecessor))
        .flatten()
        .and_then(WorkItem::current_attempt);
        match current {
            Some(attempt) => StartRelease {
                release: Release::evaluate(
                    Some(EventTime::Recorded(attempt.submitted_at)),
                    edge.lag_hours,
                    self.now(),
                ),
                attempt: Some(attempt.number),
                provisional: attempt.outcome == AttemptOutcome::Pending,
            },
            None => StartRelease {
                release,
                attempt: None,
                provisional: false,
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
