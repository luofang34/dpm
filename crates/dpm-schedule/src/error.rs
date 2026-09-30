use dpm_model::{ValidationError, WorkItemId};
use thiserror::Error;
#[derive(Debug, Error)]
/// Invalid schedule input or non-finite schedule arithmetic.
pub enum ScheduleError {
    /// Plan invariants are invalid.
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// Finite inputs overflowed during schedule arithmetic.
    #[error("schedule arithmetic overflow for work {0}")]
    ArithmeticOverflow(WorkItemId),
    #[error("dependency references missing work item {0}")]
    /// A dependency refers to work outside the plan.
    MissingWorkItem(WorkItemId),
    #[error("dependency graph contains a cycle")]
    /// Temporal constraints form a directed cycle.
    DependencyCycle,
    #[error("invalid duration for work item {0}")]
    /// An activity has a missing, negative, or non-finite duration.
    InvalidDuration(WorkItemId),
    #[error("schedule position {0} is outside the compiled dependency network")]
    /// A position read from a compiled network has no activity; the projection is inconsistent.
    UnknownPosition(usize),
    #[error("the projection needs working time beyond the supported calendar range")]
    /// A calendar has no working time within the range a projection can compile.
    CalendarRange,
    #[error("simulation requires at least one iteration")]
    /// At least one Monte Carlo iteration is required.
    NoSimulationIterations,
}
