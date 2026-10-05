//! Pure deterministic and probabilistic projections of validated execution graphs.

mod cpm;
mod error;
mod network;
mod options;
mod placement;
mod projection;
mod remaining_duration;
mod sampling;
mod simulation;

pub use cpm::{
    deterministic, deterministic_remaining, deterministic_remaining_at,
    deterministic_remaining_with, deterministic_with_durations,
};
pub use error::ScheduleError;
pub use options::RemainingOptions;
pub use placement::worked_between;
pub use projection::{ActivityCalendar, ActivitySchedule, RelationSchedule, Schedule};
pub use simulation::{
    SimulationConfig, SimulationSummary, percentile, simulate, simulate_remaining,
    simulate_remaining_at, simulate_remaining_with,
};
