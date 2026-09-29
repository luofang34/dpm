use super::{ValidationError, invalid};
use crate::{Decision, DecisionStatus, WorkItem, WorkStatus};

/// Event times must match the lifecycle that records them and occur in lifecycle order.
///
/// Ordering is the guardrail against backdating: a command whose timestamp precedes the event it
/// follows yields an invalid candidate, so the command is rejected and state stays unchanged.
pub(super) fn work(work: &WorkItem) -> Result<(), ValidationError> {
    let events = &work.execution.events;
    if !work.is_executable() && !events.is_empty() {
        return Err(invalid(
            "work events",
            work.id,
            "only tasks record execution events",
        ));
    }
    let started = matches!(
        work.execution.status,
        WorkStatus::InProgress
            | WorkStatus::Blocked
            | WorkStatus::Submitted
            | WorkStatus::Verified
            | WorkStatus::Done
    );
    let submitted = matches!(
        work.execution.status,
        WorkStatus::Submitted | WorkStatus::Verified | WorkStatus::Done
    );
    if (events.start_unrecorded && (events.started_at.is_some() || !started))
        || (events.started_at.is_some() && !started)
        || (events.submitted_at.is_some() && !submitted)
        || (events.verified_at.is_some() && !work.execution.status.satisfies_dependency())
    {
        return Err(invalid(
            "work events",
            work.id,
            format!(
                "recorded events do not match lifecycle {:?}",
                work.execution.status
            ),
        ));
    }
    let ordered = [events.started_at, events.submitted_at, events.verified_at]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if ordered
        .windows(2)
        .any(|pair| matches!(pair, [a, b] if a > b))
    {
        return Err(invalid(
            "work events",
            work.id,
            "start, submission and verification times must not precede one another",
        ));
    }
    handoffs(work)
}

fn handoffs(work: &WorkItem) -> Result<(), ValidationError> {
    if !work.is_executable() && !work.execution.handoffs.is_empty() {
        return Err(invalid("work handoff", work.id, "only tasks change owners"));
    }
    for handoff in &work.execution.handoffs {
        if handoff.from == handoff.to || handoff.reason.trim().is_empty() {
            return Err(invalid(
                "work handoff",
                work.id,
                "a handoff names two different owners and a nonempty reason",
            ));
        }
    }
    if work
        .execution
        .handoffs
        .windows(2)
        .any(|pair| matches!(pair, [a, b] if a.at > b.at))
    {
        return Err(invalid(
            "work handoff",
            work.id,
            "handoffs are recorded in time order",
        ));
    }
    releases(work)
}

fn releases(work: &WorkItem) -> Result<(), ValidationError> {
    if !work.is_executable() && !work.execution.releases.is_empty() {
        return Err(invalid("work release", work.id, "only tasks are claimed"));
    }
    if work
        .execution
        .releases
        .iter()
        .any(|r| r.reason.trim().is_empty())
    {
        return Err(invalid(
            "work release",
            work.id,
            "a release names a nonempty reason",
        ));
    }
    if work
        .execution
        .releases
        .windows(2)
        .any(|pair| matches!(pair, [a, b] if a.at > b.at))
    {
        return Err(invalid(
            "work release",
            work.id,
            "releases are recorded in time order",
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
