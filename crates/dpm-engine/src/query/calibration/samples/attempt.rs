//! What happened between a task's start and its verified submission.

use super::ExclusionReason;
use crate::{Command, Operation};
use chrono::{DateTime, Utc};
use dpm_model::{ActorKind, AttemptOutcome, Plan, WorkItem};
use std::collections::BTreeSet;

/// Working hours from the start to the verified submission, less the review waits of rejected
/// attempts in between: that wait is measured as a review, so counting it as execution too would
/// charge it twice. Blocked time still counts, because estimates are durations, not effort.
pub(super) fn actual_hours(
    plan: &Plan,
    work: &WorkItem,
    started: DateTime<Utc>,
    submitted: DateTime<Utc>,
) -> Result<f64, ExclusionReason> {
    let worked = |from, to| {
        dpm_schedule::worked_between(plan, work, from, to)
            .map_err(|_| ExclusionReason::CalendarOutOfRange)
    };
    let gross = worked(started, submitted)?;
    let mut waits = 0.0;
    for attempt in &work.execution.attempts {
        let AttemptOutcome::Rejected { at, .. } = &attempt.outcome else {
            continue;
        };
        let from = attempt.submitted_at.max(started);
        let to = (*at).min(submitted);
        if to > from {
            waits += worked(from, to)?;
        }
    }
    Ok((gross - waits).max(0.0))
}

/// Whether actors of more than one kind held the work from `start` to `submit`: whoever started,
/// claimed, submitted, or gave or received it by handoff in that span.
pub(super) fn mixed_executors(
    operations: &[&Operation],
    start: &Operation,
    submit: &Operation,
) -> bool {
    let position = |target: &Operation| operations.iter().position(|op| std::ptr::eq(*op, target));
    let (Some(first), Some(last)) = (position(start), position(submit)) else {
        return false;
    };
    let span = operations
        .get(first.min(last)..=first.max(last))
        .unwrap_or_default();
    let mut kinds: BTreeSet<ActorKind> = BTreeSet::new();
    for operation in span {
        match &operation.command {
            Command::Start { .. } | Command::Submit { .. } | Command::Claim { .. } => {
                kinds.insert(operation.actor.kind);
            }
            Command::Handoff { from, to, .. } => {
                kinds.insert(from.kind);
                kinds.insert(to.kind);
            }
            _ => {}
        }
    }
    kinds.len() > 1
}
