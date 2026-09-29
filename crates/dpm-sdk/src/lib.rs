//! DPM facade crate.
//!
//! DPM models executable project plans for humans and software agents. The authoritative
//! model is a semantic execution graph; Gantt charts, network diagrams, TUI views, and future
//! native/web applications are projections of that graph.
//!
//! [`app`] is the application layer the CLI and agent tools use: typed and JSON queries, validated
//! mutations, a cheap revision query and commit notifications. Native and web clients use it the
//! same way and leave out the `sqlite`, `registry` and `git` features where their target lacks them.

pub use dpm_app as app;
pub use dpm_engine as engine;
pub use dpm_model as model;
pub use dpm_schedule as schedule;
