//! Validated commands and consistent execution projections for humans and agents.

mod change;
pub use change::{
    AffectedWork, ApplicabilityChange, ChangePreview, EdgeRelaxation, EntityChange,
    RelaxedConstraint, propose_change,
};
mod command;
mod context;
mod execution;
mod gates;
pub use gates::{
    GateReport, ProvisionalRelease, Transition, UnmetGate, describe_applicability, gate_report,
};
mod progress;
mod query;
mod readiness;
mod scope;
pub use scope::{
    NEXT_RESULT_VERSION, NextWorkResult, OutsideScope, ScopeError, ScopeMember, ScopedCandidate,
    WorkScope, next_in_scope,
};

pub use command::{Command, EngineError, ExternalLinkRequest, Operation};
pub use execution::apply_command;
pub use query::{
    BasisReport, InapplicableWork, MAX_SCENARIOS, NextWorkCandidate, NextWorkQuery, OpenChoices,
    ScenarioForecast, StatusSummary, WorkExplanation, excluded_in_flight, explain_work,
    in_status_scope, is_unestimated, next_work, status, unestimated,
};
pub use readiness::{completion, is_ready, show_work};

pub use context::ExecutionContext;

pub use progress::{ProgressProjection, ProgressScope, ProgressSummary, progress};
