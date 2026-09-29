//! Pure deterministic and probabilistic projections of validated execution graphs.

mod cpm;
mod error;
mod network;
mod projection;
mod remaining_duration;
mod sampling;
mod simulation;

pub use cpm::{
    deterministic, deterministic_remaining, deterministic_remaining_at,
    deterministic_with_durations,
};
pub use error::ScheduleError;
pub use projection::{ActivitySchedule, Schedule};
pub use simulation::{
    SimulationConfig, SimulationSummary, simulate, simulate_remaining, simulate_remaining_at,
};
