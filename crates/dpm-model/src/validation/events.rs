use super::{ValidationError, invalid};
use crate::{Decision, DecisionStatus, WorkItem, WorkStatus};

/// Event times must match the lifecycle that records them and occur in lifecycle order.
///
/// Ordering is the guardrail against backdating: a command whose timestamp precedes the event it
/// follows yields an invalid candidate, so the command is rejected and state stays unchanged.
pub(super) fn work(work: &WorkItem) -> Result<(), ValidationError> {
    let events = &work.events;
    if !work.is_executable() && !events.is_empty() {
        return Err(invalid(
            "work events",
            work.id,
            "only tasks record execution events",
        ));
    }
    let started = matches!(
        work.status,
        WorkStatus::InProgress
            | WorkStatus::Blocked
            | WorkStatus::Submitted
            | WorkStatus::Verified
            | WorkStatus::Done
    );
    let submitted = matches!(
        work.status,
        WorkStatus::Submitted | WorkStatus::Verified | WorkStatus::Done
    );
    if (events.started_at.is_some() && !started)
        || (events.submitted_at.is_some() && !submitted)
        || (events.verified_at.is_some() && !work.status.satisfies_dependency())
    {
        return Err(invalid(
            "work events",
            work.id,
            format!("recorded events do not match lifecycle {:?}", work.status),
        ));
    }
    let ordered = [events.started_at, events.submitted_at, events.verified_at]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if ordered.windows(2).any(|pair| pair[0] > pair[1]) {
        return Err(invalid(
            "work events",
            work.id,
            "start, submission and verification times must not precede one another",
        ));
    }
    Ok(())
}

pub(super) fn decision(decision: &Decision) -> Result<(), ValidationError> {
    if decision.status == DecisionStatus::Open && decision.resolved_at.is_some() {
        return Err(invalid(
            "decision",
            decision.id,
            "open decision cannot carry a resolution time",
        ));
    }
    Ok(())
}
