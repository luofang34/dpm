//! The plan as its recorded events stood at an earlier instant.
//!
//! Event times say when something happened, which for backfilled or imported history is long
//! before the operation that recorded it, so a look back follows them rather than the operation
//! log. The current structure is kept: contracts, estimates and dependencies are the plan being
//! examined, and `history` shows what was recorded when.

use crate::*;
use chrono::{DateTime, Utc};

impl Plan {
    /// This plan with every execution, decision and waiver fact recorded after `at` withdrawn.
    ///
    /// A task's lifecycle is rolled back to the last of its own events at or before `at`: verified,
    /// a pending submission, started, claimed, or planned. Its owner then is the one a later
    /// handoff took it from or a later release gave it up from. Facts that carry no time of their
    /// own (the reported percentage and a block) are kept while the lifecycle is unchanged, a block
    /// from the latest lifecycle event it must follow, and cleared when it rolls back further,
    /// since they cannot be placed before the later event. The claim time is the current
    /// reservation's: a start clears it and a handoff or re-claim replaces it, so unstarted work
    /// looked at before that later reservation shows as planned. Events whose time was never
    /// recorded count as having occurred. Decisions resolved after `at` are open again, a
    /// replacement resolved after `at` no longer replaces anything and the decision it superseded
    /// stands as it was, and a waiver granted after `at` is withdrawn.
    #[must_use]
    pub fn as_of(&self, at: DateTime<Utc>) -> Self {
        let mut plan = self.clone();
        for work in plan.work_items.values_mut() {
            if work.is_executable() {
                execution_as_of(&mut work.execution, at);
            }
        }
        let superseded_later: Vec<DecisionId> = self
            .decisions
            .values()
            .filter(|d| d.resolved_at.is_some_and(|r| r > at))
            .filter_map(|d| d.supersedes)
            .collect();
        for decision in plan.decisions.values_mut() {
            if decision.resolved_at.is_some_and(|r| r > at) {
                decision.status = DecisionStatus::Open;
                decision.outcome = None;
                decision.resolved_at = None;
                // A replacement made later neither replaced nor gated anything yet.
                if decision.supersedes.take().is_some() {
                    decision.blocks.clear();
                }
            } else if superseded_later.contains(&decision.id) {
                decision.status = if decision.outcome.is_some() {
                    DecisionStatus::Decided
                } else {
                    DecisionStatus::Open
                };
            }
        }
        for dependency in &mut plan.dependencies {
            if dependency.waiver.as_ref().is_some_and(|w| w.at > at) {
                dependency.waiver = None;
            }
        }
        plan
    }
}

fn execution_as_of(execution: &mut ExecutionRecord, at: DateTime<Utc>) {
    if execution
        .latest_recorded_at()
        .is_none_or(|latest| latest <= at)
    {
        return;
    }
    let owner_then = owner_at(execution, at);
    let recorded = execution.clone();
    let events = &mut execution.events;
    for time in [
        &mut events.claimed_at,
        &mut events.started_at,
        &mut events.submitted_at,
        &mut events.verified_at,
    ] {
        if time.is_some_and(|t| t > at) {
            *time = None;
        }
    }
    execution.attempts.retain(|a| a.submitted_at <= at);
    for attempt in &mut execution.attempts {
        let closed_later = match &attempt.outcome {
            AttemptOutcome::Pending => false,
            AttemptOutcome::Rejected { at: closed, .. }
            | AttemptOutcome::Verified { at: closed, .. } => *closed > at,
        };
        if closed_later {
            attempt.outcome = AttemptOutcome::Pending;
        }
    }
    if execution.last_rejection.as_ref().is_some_and(|r| r.at > at) {
        execution.last_rejection = execution
            .attempts
            .iter()
            .rev()
            .find_map(|a| match &a.outcome {
                AttemptOutcome::Rejected { actor, at, reason } => Some(ReviewRejection {
                    actor: actor.clone(),
                    at: *at,
                    reason: reason.clone(),
                }),
                _ => None,
            });
    }
    // The current submission is the pending latest attempt, after a rejection and resubmission too.
    if !execution.attempts.is_empty() {
        execution.events.submitted_at = execution
            .attempts
            .last()
            .filter(|a| a.outcome == AttemptOutcome::Pending)
            .map(|a| a.submitted_at);
    }
    execution.handoffs.retain(|h| h.at <= at);
    execution.releases.retain(|r| r.at <= at);
    execution.basis.retain(|b| b.recorded_at <= at);
    let mut status = status_then(execution, &recorded);
    // A block suspends work where it stood. Every lifecycle transition needs the work unblocked, so
    // a standing block came after the latest of them; only handoffs and reviews of a basis follow.
    let suspended = if execution.events.started_at.is_some() || recorded.events.start_unrecorded {
        WorkStatus::InProgress
    } else {
        WorkStatus::Claimed
    };
    if recorded.status == WorkStatus::Blocked
        && status == suspended
        && lifecycle_recorded_at(&recorded).is_none_or(|latest| latest <= at)
    {
        status = WorkStatus::Blocked;
    }
    if status != execution.status {
        execution.status = status;
        execution.block_reason = None;
        if !matches!(status, WorkStatus::Submitted | WorkStatus::Verified) {
            execution.reported_progress_percent = 0;
        }
    }
    execution.owner = match status {
        WorkStatus::Proposed | WorkStatus::Planned => None,
        _ => owner_then,
    };
    if execution.owner.is_none() && status != WorkStatus::Proposed {
        execution.status = WorkStatus::Planned;
    }
}

/// The lifecycle the task's remaining events establish; `recorded` is the unmasked record, whose
/// lifecycle shows events that occurred without a recorded time.
fn status_then(execution: &ExecutionRecord, recorded: &ExecutionRecord) -> WorkStatus {
    let events = &execution.events;
    let pending = execution
        .attempts
        .last()
        .is_some_and(|a| a.outcome == AttemptOutcome::Pending);
    let verified_unrecorded =
        recorded.status.is_terminal() && recorded.events.verified_at.is_none();
    let start_unrecorded = recorded.events.started_at.is_none()
        && (recorded.events.start_unrecorded
            || matches!(
                recorded.status,
                WorkStatus::InProgress | WorkStatus::Submitted
            )
            || verified_unrecorded);
    if events.verified_at.is_some() || verified_unrecorded {
        recorded.status
    } else if pending {
        WorkStatus::Submitted
    } else if events.started_at.is_some() || start_unrecorded {
        WorkStatus::InProgress
    } else if events.claimed_at.is_some() {
        WorkStatus::Claimed
    } else if recorded.status == WorkStatus::Proposed {
        WorkStatus::Proposed
    } else {
        WorkStatus::Planned
    }
}

/// Latest time of a claim, start, submission, review or verification.
fn lifecycle_recorded_at(execution: &ExecutionRecord) -> Option<DateTime<Utc>> {
    let events = &execution.events;
    let reviews = execution.attempts.iter().flat_map(|a| {
        let closed = match &a.outcome {
            AttemptOutcome::Pending => None,
            AttemptOutcome::Rejected { at, .. } | AttemptOutcome::Verified { at, .. } => Some(*at),
        };
        [Some(a.submitted_at), closed]
    });
    [
        events.claimed_at,
        events.started_at,
        events.submitted_at,
        events.verified_at,
        execution.last_rejection.as_ref().map(|r| r.at),
    ]
    .into_iter()
    .chain(reviews)
    .flatten()
    .max()
}

/// Who held the task at `at`: whoever the first later handoff or release took it from.
fn owner_at(execution: &ExecutionRecord, at: DateTime<Utc>) -> Option<ActorId> {
    let handoff = execution
        .handoffs
        .iter()
        .filter(|h| h.at > at)
        .map(|h| (h.at, &h.from));
    let release = execution
        .releases
        .iter()
        .filter(|r| r.at > at)
        .map(|r| (r.at, &r.actor));
    match handoff.chain(release).min_by_key(|(when, _)| *when) {
        Some((_, holder)) => Some(holder.clone()),
        None => execution.owner.clone(),
    }
}

#[cfg(test)]
mod tests;
