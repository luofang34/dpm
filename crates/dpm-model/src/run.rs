//! Provider-independent records of agent runs and their activity.
//!
//! A run is one attempt by one executor to perform one task. It never changes the task's identity
//! or lifecycle: runs, their lifecycle transitions and their high-volume activity are observations
//! kept beside the project plan, not operations in the semantic log. A run records the contract
//! and source it observed when it started; later plan edits do not rewrite that record.

mod activity;
mod lifecycle;
mod record;
mod validation;
mod view;

pub use activity::{
    ActivityEntry, ActivityGap, ActivityInput, ActivityKind, ActivityKindParseError, ActivityPage,
    ActivityRecord, ActivityTally, LatestActivity, MAX_ACTIVITY_BATCH, MAX_ACTIVITY_TEXT_BYTES,
    NormalizedActivity,
};
pub use lifecycle::{
    LifecycleEntry, LifecycleEvent, LifecyclePage, RunState, RunStateParseError, RunTransition,
};
pub use record::{
    ContractObservation, ExactSource, Observation, ObservationParseError, RunLink, RunRecord,
    RunSession, RunSource, RunStart,
};
pub use validation::{MAX_DETAIL_BYTES, MAX_SOURCES, is_exact_commit};
pub use view::{LineageStatus, ObservedStatus, Orphan, RunSnapshot, RunView};

#[cfg(test)]
mod tests;
