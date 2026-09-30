//! Validated commands and consistent execution projections for humans and agents.

mod change;
pub use change::{
    AffectedWork, ApplicabilityChange, ChangePreview, EdgeRelaxation, EntityChange,
    RelaxedConstraint, apply_plan_change, patch, plan_change, propose_change, protected_work,
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
    Advisory, AgingWork, AppliedCalibration, AppliedFactor, AppliedReviewDelay,
    BULK_WINDOW_SECONDS, BasisReport, CalibrationReport, CalibrationRules, ClaimOutcomes,
    Durations, EstimateCalibration, EstimateSample, Exclusion, ExclusionReason, FlowReport,
    HistoryCoverage, HolderOutcomes, HourBasis, InapplicableWork, MAX_APPLIED_RATIO, MAX_SCENARIOS,
    MIN_ACTUAL_SECONDS, MIN_APPLIED_RATIO, MIN_SAMPLES, NextWorkCandidate, NextWorkQuery,
    OpenChoices, REVIEW_FLOOR_SECONDS, RatioGroup, ReasonCount, Reliability, ScenarioForecast,
    StatusSummary, THROUGHPUT_WEEKS, Throughput, WaitGroup, WaitReport, WeekCount, WorkExplanation,
    advisories, calibration, excluded_in_flight, explain_work, in_status_scope, is_unestimated,
    next_work, status, status_calibrated, unestimated,
};
pub use readiness::{completion, is_ready, show_work};

pub use context::ExecutionContext;

pub use progress::{ProgressProjection, ProgressScope, ProgressSummary, progress};
