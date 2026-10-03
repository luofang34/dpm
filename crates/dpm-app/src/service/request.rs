//! Request shapes adapters decode and hand to the application.

use dpm_engine::Command;
use dpm_model::{ActorId, LineageId, OperationId, Plan, RunId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Mutation precondition and engine command shared by CLI and agent tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRequest {
    /// Principal accountable for this operation.
    pub actor: ActorId,
    /// Last observed revision; storage rechecks it atomically.
    pub base_revision: u64,
    /// Lineage the revision was observed in; another lineage is refused, never merged.
    #[serde(default)]
    pub base_lineage: Option<LineageId>,
    /// Version 7 identity and idempotency key; minted when absent.
    #[serde(default)]
    pub operation_id: Option<OperationId>,
    /// Validated semantic mutation.
    pub command: Command,
}

/// A reviewed full proposal submitted by an adapter; the engine records only its difference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanChangeRequest {
    /// Principal applying the reviewed change; agents are refused.
    pub actor: ActorId,
    /// Last observed revision; storage rechecks it atomically.
    pub base_revision: u64,
    /// Lineage the revision was observed in; another lineage is refused, never merged.
    #[serde(default)]
    pub base_lineage: Option<LineageId>,
    /// Version 7 identity and idempotency key; minted when absent.
    #[serde(default)]
    pub operation_id: Option<OperationId>,
    /// Full proposed graph at the observed revision.
    pub plan: Box<Plan>,
    /// Human-readable purpose of the accepted scope change.
    pub reason: String,
}

/// Read contracts; all views remain derived from the execution graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "query", rename_all = "snake_case", deny_unknown_fields)]
pub enum Query {
    /// Validate and inspect a proposed plan without changing state.
    ProposeChange {
        /// Full candidate snapshot retaining the observed revision.
        plan: Box<Plan>,
    },
    /// The committed revision and its lineage, read without decoding the plan; for polling.
    Revision,
    /// Read append-only semantic operations in local sequence order.
    History {
        /// Exclusive local operation cursor; zero starts from the beginning.
        after_sequence: u64,
        /// Maximum number of entries, capped at 1000.
        limit: u16,
    },
    /// Export a validated authoritative snapshot for plan proposals.
    Export,
    /// JSON Schema of the portable plan used by exports and proposals.
    PlanSchema,
    /// A minimal valid proposal for this workspace; refused once it has projects or work.
    PlanTemplate,
    /// Map a Microsoft Project XML document onto a reviewed plan-change candidate.
    ImportMspdi {
        /// Complete MSPDI document text.
        xml: String,
        /// Existing project receiving the imported work.
        project_key: String,
        /// Prefix for keys of new work; defaults to the project key, and names the source of a
        /// document without GUIDs, which requires it.
        #[serde(default)]
        key_prefix: Option<String>,
        /// Opt-in rule matching GUID-less tasks to existing work; absent never matches.
        #[serde(default)]
        match_existing_by: Option<dpm_interchange::ExistingMatch>,
        /// Existing work keeps its priority; the report names differing source values.
        #[serde(default)]
        keep_existing_priority: bool,
        /// IANA time zone for the document's calendars; absent leaves calendars unimported.
        #[serde(default)]
        time_zone: Option<String>,
    },
    /// Write one project's work as the supported Microsoft Project XML subset.
    ExportMspdi {
        /// Project whose work is written.
        project_key: String,
    },
    /// Execution counts and optional Monte Carlo forecast.
    Status {
        /// Compute seeded uncertainty projections.
        probabilistic: bool,
        /// Apply measured estimate factors and review waits that have enough samples, and state
        /// them; absent or false leaves the forecast unchanged.
        #[serde(default)]
        calibrated: bool,
    },
    /// The remaining-work schedule projection: elapsed-hour times, float, package spans and, when
    /// requested, the seeded simulation's criticality and finish percentiles.
    Schedule {
        /// Add the seeded uncertainty projections `status` and `next` use.
        probabilistic: bool,
    },
    /// Calibration of estimates, review and decision waits, and flow metrics from history.
    Calibration,
    /// Globally ranked executable leaf tasks, narrowed to a visible query-only scope.
    Next {
        /// Requested capabilities; empty preserves the engine's unfiltered operator view.
        capabilities: BTreeSet<String>,
        /// Include seeded probabilistic criticality.
        probabilistic: bool,
        /// Maximum number of in-scope results, applied after scope filtering.
        limit: usize,
        /// Project keys whose subtrees form the project scope; empty does not filter.
        #[serde(default)]
        project_keys: BTreeSet<String>,
        /// WorkspaceAsset keys that returned work must fit; empty does not filter.
        #[serde(default)]
        asset_keys: BTreeSet<String>,
        /// Actor about to choose work; adds advice such as the claims it already holds, never
        /// changing the candidates.
        #[serde(default)]
        actor: Option<ActorId>,
    },
    /// Runs, newest first, each with its lifecycle, derived freshness and attribution; optionally
    /// only those executing one task. Runs are observations beside the plan, never project facts.
    Runs {
        /// Only runs executing this work key.
        #[serde(default)]
        key: Option<String>,
        /// Maximum number of runs, capped at 1000.
        #[serde(default = "default_page")]
        limit: u16,
    },
    /// One run with its lifecycle, derived freshness, attribution and linked operations.
    Run {
        /// Run identity.
        id: RunId,
    },
    /// Durable run lifecycle facts after a feed cursor; never pruned, so continuous.
    RunLifecycle {
        /// Exclusive feed cursor; zero starts from the beginning.
        #[serde(default)]
        after_sequence: u64,
        /// Maximum number of entries, capped at 1000.
        #[serde(default = "default_page")]
        limit: u16,
        /// Only this run's facts.
        #[serde(default)]
        run: Option<RunId>,
    },
    /// Bounded run activity after a feed cursor, independent of the lifecycle cursor; a cursor
    /// that retention outran is answered with a gap.
    RunActivity {
        /// Exclusive feed cursor; zero starts from the beginning.
        #[serde(default)]
        after_sequence: u64,
        /// Maximum number of entries, capped at 1000.
        #[serde(default = "default_page")]
        limit: u16,
        /// Only this run's records.
        #[serde(default)]
        run: Option<RunId>,
    },
    /// Work contract with projected container/milestone status.
    Show {
        /// Stable human-readable work key.
        key: String,
    },
    /// Readiness, dependencies, schedule and ranking explanation.
    Explain {
        /// Stable human-readable work key.
        key: String,
    },
}

/// Default page size of the run feeds and list, matching `history`.
fn default_page() -> u16 {
    100
}
