//! What the console reads through its adapter: a cheap revision probe and full snapshots.

use dpm_model::{LineageId, Plan};

/// Where the source stands: its committed revision and the history that revision belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRevision {
    /// Committed revision.
    pub revision: u64,
    /// Lineage of that revision; absent for a read-only preview.
    pub lineage_id: Option<LineageId>,
}

/// A plan together with the lineage of the source it was loaded from.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The loaded plan; its `revision` is the revision displayed.
    pub plan: Plan,
    /// Lineage of the source the plan was read from.
    pub lineage_id: Option<LineageId>,
}

impl Snapshot {
    /// The revision and lineage this snapshot shows.
    pub fn revision(&self) -> SourceRevision {
        SourceRevision {
            revision: self.plan.revision,
            lineage_id: self.lineage_id,
        }
    }
}

/// Adapter through which the console follows a workspace. Both calls block on I/O and run on the
/// console thread between input events, so they must stay short.
pub trait SnapshotSource {
    /// Failure reported to the operator as text.
    type Error: std::fmt::Display;

    /// The source's current revision and lineage, without loading the plan.
    fn revision_blocking(&mut self) -> Result<SourceRevision, Self::Error>;

    /// A full snapshot of the source as it stands now.
    fn snapshot_blocking(&mut self) -> Result<Snapshot, Self::Error>;
}

/// A source that never changes: the console still reloads it, and its probe always matches.
pub(crate) struct FixedPlan(pub(crate) Snapshot);

impl SnapshotSource for FixedPlan {
    type Error = std::convert::Infallible;

    fn revision_blocking(&mut self) -> Result<SourceRevision, Self::Error> {
        Ok(self.0.revision())
    }

    fn snapshot_blocking(&mut self) -> Result<Snapshot, Self::Error> {
        Ok(self.0.clone())
    }
}
