//! Where a run store's feeds stand, read in one transaction.

use crate::LineageId;
use serde::{Deserialize, Serialize};

/// The positions of a run store's durable facts.
///
/// A run's view is built from three kinds of durable fact, and each is tracked on its own: the
/// lifecycle transitions and the bounded activity, which are feeds with cursors, and the operations
/// linked to a run afterwards, which are an invalidation count. A consumer holds each separately,
/// because lifecycle facts are never pruned while activity is bounded, a link changes a run's view
/// without being either, and none of them moves when the project's operation history does. Two
/// readings with equal positions in one epoch describe the same run facts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFeedHeads {
    /// The lineage the run store is bound to, absent while no run store exists. Feed cursors are
    /// meaningful only within one epoch: a restore forks the run store to a new lineage, and its
    /// sequences continue from the backup, not from the store it was copied from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<LineageId>,
    /// Newest lifecycle feed sequence, zero when the feed is empty.
    pub lifecycle_head: u64,
    /// Newest activity feed sequence ever assigned, zero when the feed is empty.
    pub activity_head: u64,
    /// Highest activity sequence retention has removed; a cursor below it has a gap.
    pub activity_pruned_through: u64,
    /// How many operations were ever linked to a run in this epoch.
    ///
    /// Links are append-only and each distinct operation is linked once, so the count only grows
    /// and a replayed link does not move it. It is an invalidation token, not a feed: it says that
    /// some run's linked operations changed, not which, and a consumer that sees it change reads
    /// the runs it displays again. It promises no order of links and no resumable position.
    #[serde(default)]
    pub link_count: u64,
}
