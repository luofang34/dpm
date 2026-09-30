//! How long submitted work waits for review and open decisions wait for a decision.

use super::{
    ExclusionReason, HistoryIndex, MIN_SAMPLES, REVIEW_FLOOR_SECONDS, WaitGroup, WaitReport,
    ascending, elapsed_hours, exclusions,
};
use chrono::TimeDelta;
use dpm_model::{ActorKind, AttemptOutcome, DecisionStatus, Key, Plan, WorkItemId};
use dpm_schedule::percentile;
use std::collections::{BTreeMap, BTreeSet};

/// Elapsed hours from each reviewed submission to its review, by reviewer kind.
///
/// Rejected and verified attempts both count. Work whose start and submit were recorded in bulk
/// has no honest submission time, a review recorded within [`REVIEW_FLOOR_SECONDS`] of its
/// submission measures the recording rather than a wait, and work verified before attempts were
/// recorded names no reviewer, so none of these is measured.
pub(super) fn reviews(plan: &Plan, bulk: &BTreeSet<WorkItemId>) -> WaitReport {
    let mut waits: BTreeMap<ActorKind, Vec<f64>> = BTreeMap::new();
    let mut excluded: Vec<(ExclusionReason, Key)> = Vec::new();
    for work in plan.work_items.values().filter(|w| w.is_executable()) {
        if work.execution.attempts.is_empty() && work.execution.status.satisfies_dependency() {
            excluded.push((ExclusionReason::AttemptUnrecorded, work.key.clone()));
        }
        for attempt in &work.execution.attempts {
            let (reviewer, at) = match &attempt.outcome {
                AttemptOutcome::Pending => continue,
                AttemptOutcome::Rejected { actor, at, .. }
                | AttemptOutcome::Verified { actor, at } => (actor.kind, *at),
            };
            let floor = TimeDelta::seconds(REVIEW_FLOOR_SECONDS);
            if bulk.contains(&work.id) {
                excluded.push((ExclusionReason::BulkRecorded, work.key.clone()));
            } else if at - attempt.submitted_at < floor {
                excluded.push((ExclusionReason::ReviewedOnSubmission, work.key.clone()));
            } else {
                let hours = elapsed_hours(attempt.submitted_at, at);
                waits.entry(reviewer).or_default().push(hours);
            }
        }
    }
    report(waits, excluded)
}

/// Elapsed hours from a decision's opening to its resolution, by deciding actor kind.
///
/// A decision is opened by the reviewed change that added it; one present before the log begins
/// has no known opening. Replacements are created decided and never waited.
pub(super) fn decisions(plan: &Plan, index: &HistoryIndex<'_>) -> WaitReport {
    let mut waits: BTreeMap<ActorKind, Vec<f64>> = BTreeMap::new();
    let mut excluded: Vec<(ExclusionReason, Key)> = Vec::new();
    for decision in plan
        .decisions
        .values()
        .filter(|d| d.status != DecisionStatus::Open)
    {
        let reason = if decision.supersedes.is_some() {
            Err(ExclusionReason::Replacement)
        } else {
            match (decision.resolved_at, index.opened(decision.id)) {
                (None, _) => Err(ExclusionReason::ResolutionUnrecorded),
                (Some(_), None) => Err(ExclusionReason::OpenedBeforeHistory),
                (Some(resolved), Some(opened)) => index
                    .decided(decision.id)
                    .map(|decided| {
                        (
                            decided.actor.kind,
                            elapsed_hours(opened.timestamp, resolved),
                        )
                    })
                    .ok_or(ExclusionReason::NotInHistory),
            }
        };
        match reason {
            Ok((kind, hours)) => waits.entry(kind).or_default().push(hours),
            Err(reason) => excluded.push((reason, decision.key.clone())),
        }
    }
    report(waits, excluded)
}

fn report(
    waits: BTreeMap<ActorKind, Vec<f64>>,
    excluded: Vec<(ExclusionReason, Key)>,
) -> WaitReport {
    WaitReport {
        by_kind: waits
            .into_iter()
            .map(|(kind, hours)| {
                let sorted = ascending(hours);
                WaitGroup {
                    kind,
                    count: sorted.len(),
                    median_hours: percentile(&sorted, 0.5),
                    p80_hours: percentile(&sorted, 0.8),
                    sufficient: sorted.len() >= MIN_SAMPLES,
                }
            })
            .collect(),
        excluded: exclusions(excluded),
    }
}
