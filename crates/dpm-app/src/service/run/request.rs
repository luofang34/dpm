//! Request shapes for run commands, shared by the CLI and the agent tools.

use chrono::{DateTime, Utc};
use dpm_model::{
    ActivityInput, ActorId, LineageId, Observation, OperationId, RunEventId, RunId, RunSession,
    RunSource, RunState,
};
use serde::{Deserialize, Serialize};

/// Start a run on a task the executor owns.
///
/// The run id is the idempotency key: send the same id to retry safely, and omit it only when a
/// retry is not needed. No `base_revision` is involved: a run observes the plan, it never edits it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunStartRequest {
    /// Principal recording the run: the executor, or a human or service acting for it.
    pub actor: ActorId,
    /// Key of the task being executed.
    pub work_key: String,
    /// The principal doing the work; defaults to `actor`.
    #[serde(default)]
    pub executor: Option<ActorId>,
    /// Version 7 run identity; minted when absent.
    #[serde(default)]
    pub run_id: Option<RunId>,
    /// Run that spawned this one.
    #[serde(default)]
    pub parent: Option<RunId>,
    /// Provider session this run belongs to.
    #[serde(default)]
    pub session: Option<RunSession>,
    /// How completely the executor is observed; only a service may say `managed`.
    pub observation: Observation,
    /// Exact source references.
    #[serde(default)]
    pub sources: Vec<RunSource>,
    /// When the executor says the run began; a claim, never used to judge freshness.
    #[serde(default)]
    pub observed_at: Option<DateTime<Utc>>,
    /// Lineage the caller observed; another lineage is refused.
    #[serde(default)]
    pub base_lineage: Option<LineageId>,
}

/// Record a run's lifecycle transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunReportRequest {
    /// Principal reporting: the run's executor, or a human or service.
    pub actor: ActorId,
    /// The run.
    pub run: RunId,
    /// The state it moves to.
    pub state: RunState,
    /// Version 7 transition identity and idempotency key; minted when absent.
    #[serde(default)]
    pub event_id: Option<RunEventId>,
    /// Why, in the reporter's words.
    #[serde(default)]
    pub detail: Option<String>,
    /// When the executor says it happened; a claim, never used to judge freshness.
    #[serde(default)]
    pub observed_at: Option<DateTime<Utc>>,
    /// Lineage the caller observed; another lineage is refused.
    #[serde(default)]
    pub base_lineage: Option<LineageId>,
}

/// Record activity for runs. Activity is telemetry: it changes no lifecycle state and no plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunActivityRequest {
    /// Principal reporting: the run's executor, or a human or service.
    pub actor: ActorId,
    /// One to 100 records; all are recorded or none.
    pub entries: Vec<ActivityInput>,
    /// Lineage the caller observed; another lineage is refused.
    #[serde(default)]
    pub base_lineage: Option<LineageId>,
}

/// Link a committed project operation to the run that performed it.
///
/// This is its own explicit step, never a side effect of a project command: the operation is
/// already committed, and the link is refused unless it is provably the run's own.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunLinkRequest {
    /// Principal linking: the run's executor, or a human or service.
    pub actor: ActorId,
    /// The run.
    pub run: RunId,
    /// The committed operation, as returned by the project command.
    pub operation: OperationId,
    /// Lineage the caller observed; another lineage is refused.
    #[serde(default)]
    pub base_lineage: Option<LineageId>,
}

/// A run command, the one entry point the JSON adapters call.
#[derive(Debug, Clone)]
pub enum RunCommand {
    /// Start a run.
    Start(RunStartRequest),
    /// Record a lifecycle transition.
    Report(RunReportRequest),
    /// Record activity.
    Activity(RunActivityRequest),
    /// Link an operation to a run.
    Link(RunLinkRequest),
}
