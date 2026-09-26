//! DPM facade crate.
//!
//! DPM models executable project plans for humans and software agents. The authoritative
//! model is a semantic execution graph; Gantt charts, network diagrams, TUI views, and future
//! native/web applications are projections of that graph.

pub use dpm_engine as engine;
pub use dpm_model as model;
pub use dpm_schedule as schedule;
