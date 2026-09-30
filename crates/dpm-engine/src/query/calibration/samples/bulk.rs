//! Which starts and submissions were recorded after the fact in one sitting.

use super::super::BULK_WINDOW_SECONDS;
use super::super::history::{HistoryIndex, backfilled};
use crate::{Command, Operation};
use dpm_model::WorkItemId;

/// Whether the task's latest start and the first submission after it were recorded in bulk:
/// neither names an occurrence time, and they and the claim (or handoff) that gave the holder the
/// work were committed within [`BULK_WINDOW_SECONDS`] of each other.
///
/// A claim committed well before its start and submit means the holder had the work that long, so
/// a quick start-to-submit is genuinely short work, not a record typed in afterwards. When the
/// claim predates the log only the start and submit can be compared.
pub(super) fn bulk_recorded(index: &HistoryIndex<'_>, work: WorkItemId) -> bool {
    let operations = index.of(work);
    let Some(position) = operations
        .iter()
        .rposition(|op| matches!(op.command, Command::Start { .. }))
    else {
        return false;
    };
    let claim = operations.get(..position).and_then(|before| {
        before
            .iter()
            .rev()
            .find(|op| matches!(op.command, Command::Claim { .. } | Command::Handoff { .. }))
    });
    let mut after = operations.iter().skip(position);
    let (Some(start), Some(submit)) = (
        after.next(),
        after.find(|op| matches!(op.command, Command::Submit { .. })),
    ) else {
        return false;
    };
    let together = |a: &Operation, b: &Operation| {
        (b.timestamp - a.timestamp).num_seconds().abs() <= BULK_WINDOW_SECONDS
    };
    backfilled(&start.command).is_none()
        && backfilled(&submit.command).is_none()
        && together(start, submit)
        && claim.is_none_or(|claim| together(claim, start) && together(claim, submit))
}
