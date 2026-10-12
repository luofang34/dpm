//! Pure deterministic and probabilistic projections of validated execution graphs.

mod cpm;
mod drag;
mod error;
mod network;
mod options;
mod pert;
mod placement;
mod projection;
mod remaining_duration;
mod sampling;
mod simulation;

pub use cpm::{
    deterministic, deterministic_remaining, deterministic_remaining_at,
    deterministic_remaining_with, deterministic_with_durations,
};
pub use drag::{drag, drag_remaining_at};
pub use error::ScheduleError;
pub use options::RemainingOptions;
pub use pert::{
    PERT_APPROXIMATION_NOTE, PertApproximation, pert_approximation,
    pert_approximation_remaining_at, pert_approximation_remaining_with, pert_variance_hours2,
};
pub use placement::worked_between;
pub use projection::{ActivityCalendar, ActivitySchedule, RelationSchedule, Schedule};
pub use simulation::{
    MilestoneFinish, SimulationConfig, SimulationSummary, percentile, simulate, simulate_remaining,
    simulate_remaining_at, simulate_remaining_with,
};
