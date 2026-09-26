use super::{ValidationError, invalid};
use crate::{
    AttemptOutcome, DependencyKind, Plan, StartBasis, SubmissionAttempt, WorkItem, WorkKind,
    WorkStatus,
};

/// Provisional edges, submission attempts and recorded bases must describe one consistent history.
pub(super) fn validate(plan: &Plan) -> Result<(), ValidationError> {
    for edge in plan
        .dependencies
        .iter()
        .filter(|e| e.start_basis == StartBasis::Provisional)
    {
        // Only a task has submission attempts to rely on, and only a task starts; SS/SF already
        // wait for a start and FF gates finishing, which provisional execution never relaxes.
        let tasks = [edge.predecessor, edge.successor].iter().all(|id| {
            plan.work_items
                .get(id)
                .is_some_and(|w| w.kind == WorkKind::Task)
        });
        if edge.kind != DependencyKind::FinishStart || !tasks {
            return Err(invalid(
                "dependency",
                edge.id,
                "a provisional start basis applies only to a finish-to-start edge between tasks",
            ));
        }
    }
    for work in plan.work_items.values() {
        attempts(work)?;
        basis(plan, work)?;
    }
    Ok(())
}

fn attempts(work: &WorkItem) -> Result<(), ValidationError> {
    let Some(last) = work.attempts.last() else {
        return Ok(());
    };
    let fail = |reason: &str| Err(invalid("submission attempts", work.id, reason));
    if !work.is_executable() {
        return fail("only tasks record submission attempts");
    }
    for (index, attempt) in work.attempts.iter().enumerate() {
        if usize::try_from(attempt.number).ok() != index.checked_add(1) {
            return fail("attempts are numbered consecutively from 1");
        }
        if !std::ptr::eq(attempt, last) && !is_rejected(attempt) {
            return fail("every attempt before the latest must be rejected");
        }
        if review_time(attempt).is_some_and(|at| at < attempt.submitted_at) {
            return fail("an attempt cannot be reviewed before it was submitted");
        }
    }
    if work
        .attempts
        .windows(2)
        .any(|pair| review_time(&pair[0]).is_some_and(|at| pair[1].submitted_at < at))
    {
        return fail("an attempt cannot be submitted before the previous one was rejected");
    }
    let consistent = match &last.outcome {
        AttemptOutcome::Pending => {
            work.status == WorkStatus::Submitted
                && work.events.submitted_at == Some(last.submitted_at)
        }
        AttemptOutcome::Rejected { .. } => {
            matches!(work.status, WorkStatus::InProgress | WorkStatus::Blocked)
        }
        AttemptOutcome::Verified { at, .. } => {
            work.status.satisfies_dependency() && work.events.verified_at == Some(*at)
        }
    };
    if !consistent {
        return fail("the latest attempt does not match the recorded lifecycle and events");
    }
    Ok(())
}

fn is_rejected(attempt: &SubmissionAttempt) -> bool {
    matches!(attempt.outcome, AttemptOutcome::Rejected { .. })
}

fn review_time(attempt: &SubmissionAttempt) -> Option<chrono::DateTime<chrono::Utc>> {
    match &attempt.outcome {
        AttemptOutcome::Pending => None,
        AttemptOutcome::Rejected { at, .. } | AttemptOutcome::Verified { at, .. } => Some(*at),
    }
}

fn basis(plan: &Plan, work: &WorkItem) -> Result<(), ValidationError> {
    if work.basis.is_empty() {
        return Ok(());
    }
    let Some(started) = work.events.started_at else {
        return Err(invalid(
            "dependency basis",
            work.id,
            "only started tasks record a dependency basis",
        ));
    };
    let mut previous = started;
    for entry in &work.basis {
        let edge = plan.find_dependency(entry.dependency);
        let attempt = plan
            .work_items
            .get(&entry.predecessor)
            .and_then(|p| p.attempt(entry.attempt));
        // Reviewed changes cannot remove an edge into started work, but derived projections such
        // as the remaining schedule drop edges that no longer bound anything; the attempt
        // reference itself must always resolve.
        let refers = edge.is_none_or(|e| {
            e.predecessor == entry.predecessor
                && e.successor == work.id
                && e.start_basis == StartBasis::Provisional
        });
        if !refers || attempt.is_none() {
            return Err(invalid(
                "dependency basis",
                work.id,
                format!(
                    "basis on {} must name a provisional edge into this task and an existing attempt of its predecessor",
                    entry.dependency
                ),
            ));
        }
        if entry.recorded_at < previous {
            return Err(invalid(
                "dependency basis",
                work.id,
                "a basis is recorded at or after the start and in time order",
            ));
        }
        if let crate::BasisSource::Revalidation { actor, reason } = &entry.source
            && (actor.kind == crate::ActorKind::Agent
                || reason.trim().is_empty()
                || work.owner.as_ref() == Some(actor))
        {
            return Err(invalid(
                "dependency basis",
                work.id,
                "revalidation requires an independent human or service reviewer and a reason",
            ));
        }
        previous = entry.recorded_at;
    }
    Ok(())
}
