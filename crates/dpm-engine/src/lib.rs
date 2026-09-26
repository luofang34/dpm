//! Validated commands and consistent execution projections for humans and agents.

mod change;
pub use change::{AffectedWork, ChangePreview, EntityChange, propose_change};
mod command;
mod context;
mod execution;
mod gates;
pub use gates::{GateReport, UnmetGate, gate_report};
mod progress;
mod query;
mod readiness;

pub use command::{Command, EngineError, Operation};
pub use execution::apply_command;
pub use query::{
    NextWorkCandidate, NextWorkQuery, StatusSummary, WorkExplanation, explain_work, next_work,
    status,
};
pub use readiness::{completion, decisions_resolved, dependencies_satisfied, is_ready, show_work};

pub use context::ExecutionContext;

pub use progress::{ProgressProjection, ProgressSummary, progress};
