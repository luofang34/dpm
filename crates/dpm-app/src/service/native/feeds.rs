//! The bounded, resumable poll over the project and run feeds.

use super::{
    error::NativeError,
    handler::MAX_ATTEMPTS,
    protocol::{
        Attachment, ChangesResult, Cursors, FeedCursor, FeedDelta, FeedStatus, LinkMark,
        LinkSignal, ProjectCursor, ProjectWatermark, ResetReason, Watermark,
    },
};
use crate::Application;

/// Entries per feed page when a request names none.
const DEFAULT_PAGE: u16 = 100;

/// Most entries per feed page.
const MAX_PAGE: u16 = 1000;

impl Application {
    /// Everything after the client's cursors, one bounded page per feed it follows.
    ///
    /// A cursor that cannot be followed is a `reset`, not an error: the client's state belongs to
    /// another lineage or epoch, or to a history the source no longer holds, and it re-seeds. The
    /// returned watermark is read after the pages, so it is never older than they are and `more`
    /// is exact.
    pub(super) fn changes_blocking(
        &self,
        since: &Cursors,
        limit: Option<u16>,
        attached: Option<Attachment>,
    ) -> Result<ChangesResult, NativeError> {
        self.check_attached_blocking(attached, false)?;
        let limit = limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
        let evaluated_at = self.clock.now();
        for attempt in 1..=MAX_ATTEMPTS {
            let before = self.watermark_blocking()?;
            let project = since
                .project
                .map(|cursor| self.project_delta_blocking(cursor, limit, &before.project))
                .transpose()?;
            let lifecycle = since
                .lifecycle
                .map(|cursor| self.lifecycle_delta_blocking(cursor, limit, &before))
                .transpose()?;
            let activity = since
                .activity
                .map(|cursor| self.activity_delta_blocking(cursor, limit, &before))
                .transpose()?;
            let watermark = self.watermark_blocking()?;
            // Heads only grow while pages are read, which `more` accounts for. A different lineage
            // or epoch means the pages and the cursors built from this watermark would disagree.
            if watermark.project.lineage_id == before.project.lineage_id
                && watermark.runs.epoch == before.runs.epoch
            {
                return Ok(ChangesResult {
                    evaluated_at,
                    project: project.map(|delta| settle(delta, watermark.project.history_head)),
                    lifecycle: lifecycle.map(|delta| settle(delta, watermark.runs.lifecycle_head)),
                    activity: activity.map(|delta| settle(delta, watermark.runs.activity_head)),
                    // From the same reading as the watermark, so the count to adopt and the
                    // watermark a client keeps never disagree about which links it has seen.
                    links: since.links.map(|mark| link_signal(mark, &watermark.runs)),
                    watermark,
                });
            }
            tracing::debug!(attempt, "the history changed identity during a native poll");
        }
        Err(NativeError::Changing {
            attempts: MAX_ATTEMPTS,
        })
    }

    fn project_delta_blocking(
        &self,
        cursor: ProjectCursor,
        limit: u16,
        at: &ProjectWatermark,
    ) -> Result<FeedDelta<dpm_store::HistoryEntry>, NativeError> {
        if cursor.lineage_id != at.lineage_id {
            return Ok(reset(ResetReason::LineageChanged, cursor.after_sequence));
        }
        if cursor.after_sequence > at.history_head {
            return Ok(reset(ResetReason::CursorAhead, cursor.after_sequence));
        }
        let page = self.history_blocking(cursor.after_sequence, limit)?;
        Ok(delta(page.entries, page.next_after_sequence, None))
    }

    fn lifecycle_delta_blocking(
        &self,
        cursor: FeedCursor,
        limit: u16,
        at: &Watermark,
    ) -> Result<FeedDelta<dpm_model::LifecycleEntry>, NativeError> {
        if !same_epoch(&cursor, &at.runs) {
            return Ok(reset(ResetReason::EpochChanged, cursor.after_sequence));
        }
        if cursor.after_sequence > at.runs.lifecycle_head {
            return Ok(reset(ResetReason::CursorAhead, cursor.after_sequence));
        }
        let page = self.run_lifecycle_page_blocking(cursor.after_sequence, limit, None)?;
        Ok(delta(page.entries, page.next_after_sequence, None))
    }

    fn activity_delta_blocking(
        &self,
        cursor: FeedCursor,
        limit: u16,
        at: &Watermark,
    ) -> Result<FeedDelta<dpm_model::ActivityEntry>, NativeError> {
        if !same_epoch(&cursor, &at.runs) {
            return Ok(reset(ResetReason::EpochChanged, cursor.after_sequence));
        }
        if cursor.after_sequence > at.runs.activity_head {
            return Ok(reset(ResetReason::CursorAhead, cursor.after_sequence));
        }
        let page = self.run_activity_page_blocking(cursor.after_sequence, limit, None)?;
        Ok(delta(page.entries, page.next_after_sequence, page.gap))
    }
}

/// Whether a run-feed cursor belongs to the feed's current epoch.
///
/// A cursor that has read nothing, no epoch and sequence zero, is valid in every epoch: a client
/// that attached before any run existed starts from the beginning of whichever store appears.
fn same_epoch(cursor: &FeedCursor, heads: &dpm_model::RunFeedHeads) -> bool {
    cursor.epoch == heads.epoch || (cursor.epoch.is_none() && cursor.after_sequence == 0)
}

/// Compare a client's link mark with the count read with the feed heads.
///
/// The count is a row count, not a revision: it cannot wrap, only grows, and a mark above it means
/// the source holds fewer links than the client saw, which is a rollback.
fn link_signal(mark: LinkMark, heads: &dpm_model::RunFeedHeads) -> LinkSignal {
    let count = heads.link_count;
    let status = if mark.epoch != heads.epoch && !(mark.epoch.is_none() && mark.count == 0) {
        FeedStatus::Reset {
            reason: ResetReason::EpochChanged,
        }
    } else if mark.count > count {
        FeedStatus::Reset {
            reason: ResetReason::CursorAhead,
        }
    } else {
        FeedStatus::Continue
    };
    LinkSignal {
        changed: matches!(status, FeedStatus::Continue) && mark.count != count,
        status,
        count,
    }
}

fn reset<T>(reason: ResetReason, after_sequence: u64) -> FeedDelta<T> {
    FeedDelta {
        status: FeedStatus::Reset { reason },
        entries: Vec::new(),
        next_after_sequence: after_sequence,
        more: false,
        gap: None,
    }
}

fn delta<T>(entries: Vec<T>, next: u64, gap: Option<dpm_model::ActivityGap>) -> FeedDelta<T> {
    FeedDelta {
        status: FeedStatus::Continue,
        entries,
        next_after_sequence: next,
        more: false,
        gap,
    }
}

/// Fill in `more` from the head read after the page: the client is caught up exactly when its next
/// cursor is the head.
fn settle<T>(mut delta: FeedDelta<T>, head: u64) -> FeedDelta<T> {
    if matches!(delta.status, FeedStatus::Continue) {
        delta.more = delta.next_after_sequence != head;
    }
    delta
}
