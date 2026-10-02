//! Deriving what can be said about a run at one clock reading.
//!
//! The durable lifecycle is never edited by silence. Staleness is computed from the time DPM last
//! received anything from the run, on DPM's own clock; a time the executor reports about itself is
//! kept but never consulted, so a run cannot stay fresh by claiming a future instant.

use chrono::{DateTime, TimeDelta, Utc};
use dpm_model::{
    LineageId, LineageStatus, ObservedStatus, Orphan, Plan, RunSnapshot, RunState, RunView,
    WorkStatus,
};

/// How long a working or waiting run may go unheard before it is reported stale.
pub const STALE_AFTER: TimeDelta = TimeDelta::seconds(300);

/// Compute a run's view at `now`.
///
/// `lineage` is the history the project store continues (`None` without a store) and `plan` is the
/// snapshot read from it, used only to notice that an unfinished run's task has moved on.
#[must_use]
pub fn project(
    snapshot: RunSnapshot,
    plan: Option<&Plan>,
    lineage: Option<LineageId>,
    now: DateTime<Utc>,
) -> RunView {
    let RunSnapshot {
        record,
        last,
        activity,
        operations,
    } = snapshot;
    let current = super::same_history(record.contract.lineage_id, lineage);
    let last_receipt_at = activity.latest.as_ref().map_or(last.recorded_at, |latest| {
        latest.recorded_at.max(last.recorded_at)
    });
    let silent = now.signed_duration_since(last_receipt_at) >= STALE_AFTER;
    let status = match last.state {
        RunState::Failed => ObservedStatus::Failed,
        RunState::Interrupted => ObservedStatus::Interrupted,
        RunState::Completed => ObservedStatus::Completed,
        _ if !current => ObservedStatus::Unknown,
        _ if silent => ObservedStatus::Stale,
        RunState::Working => ObservedStatus::Working,
        RunState::Waiting => ObservedStatus::Waiting,
    };
    let stale_at = matches!(status, ObservedStatus::Working | ObservedStatus::Waiting)
        .then(|| super::stale_after(last_receipt_at))
        .flatten();
    let orphan = (!last.state.is_terminal() && current)
        .then(|| plan.and_then(|plan| orphan(&record, plan)))
        .flatten();
    RunView {
        state: last.state,
        state_since: last.recorded_at,
        state_detail: last.detail,
        status,
        last_receipt_at,
        stale_at,
        activity,
        lineage: if current {
            LineageStatus::Current
        } else {
            LineageStatus::Foreign
        },
        orphan,
        operations,
        evaluated_at: now,
        run: record,
    }
}

fn orphan(record: &dpm_model::RunRecord, plan: &Plan) -> Option<Orphan> {
    let Some(work) = plan.work_items.get(&record.work) else {
        return Some(Orphan::WorkMissing);
    };
    if work.execution.owner.as_ref() != Some(&record.executor) {
        return Some(Orphan::OwnerChanged {
            owner: work.execution.owner.clone(),
        });
    }
    match work.execution.status {
        WorkStatus::Claimed | WorkStatus::InProgress => None,
        status => Some(Orphan::NotExecuting { status }),
    }
}
