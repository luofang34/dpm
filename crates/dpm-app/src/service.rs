use crate::AppError;
use chrono::Utc;
use dpm_engine::{
    Command, NextWorkQuery, Operation, WorkScope, apply_command, explain_work, next_in_scope,
    show_work, status,
};
use dpm_model::{ActorId, DecisionId, DependencyId, Plan, WorkItemId};
use dpm_store::SqliteStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

/// Application wire contract version, independent of terminal display text.
pub const API_VERSION: u32 = 8;

/// Mutation precondition and engine command shared by CLI and agent tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRequest {
    /// Principal accountable for this operation.
    pub actor: ActorId,
    /// Last observed revision; storage rechecks it atomically.
    pub base_revision: u64,
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
        /// Resource keys that returned work must fit; empty does not filter.
        #[serde(default)]
        resource_keys: BTreeSet<String>,
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

/// A query's observed revision and stable structured data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResponse {
    /// Wire contract version.
    pub api_version: u32,
    /// Revision used to compute this view.
    pub revision: u64,
    /// The same object emitted by the corresponding CLI --json command.
    pub data: Value,
}

/// Shared application service; adapters have no writable store access.
pub struct Application {
    backing: Backing,
    pub(crate) project_root: Option<PathBuf>,
    pub(crate) project_resource: Option<dpm_model::ResourceId>,
}
enum Backing {
    Database(SqliteStore),
    Preview(Box<Plan>),
}

impl Application {
    /// Construct a validated read-only preview without opening a database.
    pub fn preview(plan: Plan) -> Result<Self, AppError> {
        plan.validate().map_err(dpm_store::StoreError::from)?;
        Ok(Self {
            project_root: None,
            project_resource: None,
            backing: Backing::Preview(Box::new(plan)),
        })
    }
    /// Whether this source refuses every state-changing operation.
    pub fn is_read_only(&self) -> bool {
        matches!(self.backing, Backing::Preview(_))
    }
    /// Reject operations on preview sources before adapter preparation or persistence.
    pub fn ensure_writable(&self) -> Result<(), AppError> {
        if self.is_read_only() {
            Err(AppError::ReadOnlyProject)
        } else {
            Ok(())
        }
    }
    /// Initialize local authoritative state through the shared application boundary.
    pub fn initialize_blocking(path: &Path, plan: &Plan) -> Result<Self, AppError> {
        plan.validate().map_err(dpm_store::StoreError::from)?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| crate::ProjectError::Io {
                action: "create database directory",
                path: parent.into(),
                source,
            })?;
        }
        let mut store = SqliteStore::open_blocking(path)?;
        store.initialize_blocking(plan)?;
        Ok(Self {
            project_root: None,
            project_resource: None,
            backing: Backing::Database(store),
        })
    }
    /// Open an existing database without accidentally creating a workspace on a read request.
    pub fn open_blocking(path: impl AsRef<Path>) -> Result<Self, AppError> {
        Ok(Self {
            project_root: None,
            project_resource: None,
            backing: Backing::Database(SqliteStore::open_existing_blocking(path)?),
        })
    }
    /// Create a disposable workspace for tests or embedded clients.
    pub fn in_memory_blocking(plan: &Plan) -> Result<Self, AppError> {
        let mut store = SqliteStore::in_memory_blocking()?;
        store.initialize_blocking(plan)?;
        Ok(Self {
            project_root: None,
            project_resource: None,
            backing: Backing::Database(store),
        })
    }
    /// Load a validated read-only snapshot; edits to this copy cannot change persistence.
    pub fn plan_blocking(&self) -> Result<Plan, AppError> {
        match &self.backing {
            Backing::Database(store) => store.load_blocking()?.ok_or(AppError::NotInitialized),
            Backing::Preview(plan) => Ok(plan.as_ref().clone()),
        }
    }
    /// Reopen the selected source for an explicit UI reload without changing workspace identity or mode.
    pub fn refreshed_plan_blocking(&self) -> Result<Plan, AppError> {
        let current = self.plan_blocking()?;
        let Some(root) = &self.project_root else {
            return Ok(current);
        };
        let source = crate::ProjectLocation::at_blocking(root)?.open_blocking()?;
        let next = source.plan_blocking()?;
        if source.is_read_only() != self.is_read_only() || next.workspace.id != current.workspace.id
        {
            return Err(AppError::InvalidRequest(
                "source identity or mode changed; reopen explicitly".into(),
            ));
        }
        Ok(next)
    }
    /// Resolve a work key to its stable UUID.
    pub fn work_id_blocking(&self, key: &str) -> Result<WorkItemId, AppError> {
        self.plan_blocking()?
            .find_work_by_key(key)
            .map(|w| w.id)
            .ok_or_else(|| AppError::UnknownWork(key.into()))
    }
    /// Resolve a decision key to its stable UUID.
    pub fn decision_id_blocking(&self, key: &str) -> Result<DecisionId, AppError> {
        self.plan_blocking()?
            .find_decision_by_key(key)
            .map(|decision| decision.id)
            .ok_or_else(|| AppError::UnknownDecision(key.into()))
    }
    /// Resolve a dependency identity string to an existing edge in the current plan.
    pub fn dependency_id_blocking(&self, id: &str) -> Result<DependencyId, AppError> {
        // A malformed identity cannot name an edge, so it is reported exactly like an absent one.
        let parsed = serde_json::from_value::<DependencyId>(Value::from(id.trim())).ok();
        let plan = self.plan_blocking()?;
        parsed
            .and_then(|parsed| plan.find_dependency(parsed))
            .map(|edge| edge.id)
            .ok_or_else(|| AppError::UnknownDependency(id.into()))
    }
    /// Compute one query from a consistent snapshot.
    pub fn query_blocking(&self, query: Query) -> Result<QueryResponse, AppError> {
        if let Query::History {
            after_sequence,
            limit,
        } = query
        {
            let page = match &self.backing {
                Backing::Database(store) => store.history_blocking(after_sequence, limit)?,
                Backing::Preview(plan) => dpm_store::HistoryPage {
                    revision: plan.revision,
                    entries: Vec::new(),
                    next_after_sequence: after_sequence,
                },
            };
            return Ok(QueryResponse {
                api_version: API_VERSION,
                revision: page.revision,
                data: serde_json::to_value(page)?,
            });
        }
        let plan = self.plan_blocking()?;
        // One clock reading per response keeps every gate, completion and schedule in it consistent.
        let now = Utc::now();
        let data = match query {
            Query::History { .. } => {
                return Err(AppError::InvalidRequest(
                    "history requires its consistent transaction".into(),
                ));
            }
            Query::ProposeChange { plan: proposed } => {
                serde_json::to_value(dpm_engine::propose_change(&plan, &proposed)?)?
            }
            Query::Export => serde_json::to_value(&plan)?,
            Query::PlanSchema => crate::plan_schema()?,
            Query::PlanTemplate => crate::authoring::plan_template(&plan)?,
            interchange @ (Query::ImportMspdi { .. } | Query::ExportMspdi { .. }) => {
                crate::interchange::query(&plan, interchange)?
            }
            Query::Status { probabilistic } => {
                serde_json::to_value(status(&plan, probabilistic, now)?)?
            }
            Query::Next {
                capabilities,
                probabilistic,
                limit,
                project_keys,
                resource_keys,
            } => {
                let scope = WorkScope::resolve(
                    &plan,
                    project_keys.iter().map(String::as_str),
                    resource_keys.iter().map(String::as_str),
                )?;
                let query = NextWorkQuery {
                    capabilities,
                    use_probabilistic_criticality: probabilistic,
                };
                serde_json::to_value(next_in_scope(&plan, &query, &scope, limit, now)?)?
            }
            Query::Show { key } => {
                let work = plan
                    .find_work_by_key(&key)
                    .ok_or(AppError::UnknownWork(key))?;
                let mut data = serde_json::to_value(show_work(&plan, work.id, now)?)?;
                data["progress"] =
                    serde_json::to_value(dpm_engine::progress(&plan, now)?.work[&work.id])?;
                data
            }
            Query::Explain { key } => {
                let work = plan
                    .find_work_by_key(&key)
                    .ok_or(AppError::UnknownWork(key))?;
                serde_json::to_value(explain_work(&plan, work.id, now)?)?
            }
        };
        Ok(QueryResponse {
            api_version: API_VERSION,
            revision: plan.revision,
            data,
        })
    }
    /// Apply and atomically persist one command. Any failure leaves the database unchanged.
    pub fn execute_blocking(&mut self, request: CommandRequest) -> Result<Operation, AppError> {
        let CommandRequest {
            actor,
            base_revision,
            command,
        } = request;
        self.commit_blocking(base_revision, |plan, at, id| {
            apply_command(plan, actor, command, at, id)
        })
    }
    /// Apply a reviewed full proposal; the operation records only its entity-level difference.
    pub fn apply_plan_change_blocking(
        &mut self,
        request: PlanChangeRequest,
    ) -> Result<Operation, AppError> {
        let PlanChangeRequest {
            actor,
            base_revision,
            plan: proposed,
            reason,
        } = request;
        self.commit_blocking(base_revision, |plan, at, id| {
            dpm_engine::apply_plan_change(plan, actor, &proposed, reason, at, id)
        })
    }
    fn commit_blocking(
        &mut self,
        base_revision: u64,
        apply: impl FnOnce(
            &mut Plan,
            chrono::DateTime<Utc>,
            dpm_model::OperationId,
        ) -> Result<Operation, dpm_engine::EngineError>,
    ) -> Result<Operation, AppError> {
        self.ensure_writable()?;
        let mut plan = self.plan_blocking()?;
        if base_revision != plan.revision {
            return Err(AppError::Conflict {
                expected: base_revision,
                actual: plan.revision,
            });
        }
        let operation = apply(&mut plan, Utc::now(), dpm_model::OperationId::new())?;
        if let Backing::Database(store) = &mut self.backing {
            store.persist_blocking(&plan, &operation)?;
        }
        Ok(operation)
    }
}

mod recovery;
pub use recovery::{restore_store_blocking, store_path_blocking, verify_store_blocking};

#[cfg(test)]
mod tests;
