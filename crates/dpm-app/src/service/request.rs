//! Request shapes adapters decode and hand to the application.

use dpm_engine::Command;
use dpm_model::{ActorId, LineageId, OperationId, Plan};
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
    },
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
