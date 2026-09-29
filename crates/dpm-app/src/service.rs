use crate::AppError;
use dpm_engine::Command;
use dpm_model::{DecisionId, DependencyId, LineageId, OperationId, Plan, WorkItemId};
#[cfg(feature = "sqlite")]
use dpm_store::SqliteStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// Application wire contract version, independent of terminal display text.
pub const API_VERSION: u32 = 12;

mod request;
pub use request::{CommandRequest, PlanChangeRequest, Query};

/// A query's observed revision and stable structured data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResponse {
    /// Wire contract version.
    pub api_version: u32,
    /// Revision used to compute this view.
    pub revision: u64,
    /// Lineage the revision belongs to; absent for a read-only preview, which has no store.
    pub lineage_id: Option<LineageId>,
    /// The same object emitted by the corresponding CLI --json command.
    pub data: Value,
}

/// Shared application service; adapters have no writable store access.
///
/// An `Application` is [`Send`] but not [`Sync`]: it owns one SQLite connection and its decode
/// cache, which must not be used from two threads at once. A client either keeps it on one owner
/// thread or actor and sends requests to it, or shares it as `Arc<Mutex<Application>>`; there is
/// no process-wide instance. Separate processes, or separate `Application`s on one store, coordinate
/// through the store's revision checks and are told apart by [`Application::revision_blocking`].
pub struct Application {
    backing: Backing,
    pub(crate) project_root: Option<PathBuf>,
    pub(crate) project_asset: Option<dpm_model::AssetId>,
    clock: QueryClock,
    watchers: watch::Watchers,
}
enum Backing {
    #[cfg(feature = "sqlite")]
    Database(SqliteStore),
    Preview(Box<Plan>),
}

impl Application {
    fn with_backing(backing: Backing) -> Self {
        Self {
            backing,
            project_root: None,
            project_asset: None,
            clock: QueryClock::System,
            watchers: watch::Watchers::default(),
        }
    }
    /// Construct a validated read-only preview without opening a database.
    pub fn preview(plan: Plan) -> Result<Self, AppError> {
        plan.validate().map_err(dpm_store::StoreError::from)?;
        Ok(Self::with_backing(Backing::Preview(Box::new(plan))))
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
    /// Load a validated read-only snapshot; edits to this copy cannot change persistence.
    pub fn plan_blocking(&self) -> Result<Plan, AppError> {
        match &self.backing {
            #[cfg(feature = "sqlite")]
            Backing::Database(store) => store.load_blocking()?.ok_or(AppError::NotInitialized),
            Backing::Preview(plan) => Ok(plan.as_ref().clone()),
        }
    }
    /// The writable history the selected store continues; `None` for a read-only preview.
    pub fn lineage_blocking(&self) -> Result<Option<LineageId>, AppError> {
        match &self.backing {
            #[cfg(feature = "sqlite")]
            Backing::Database(store) => Ok(store.lineage_blocking()?.map(|l| l.lineage_id)),
            Backing::Preview(_) => Ok(None),
        }
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
    /// Apply and atomically persist one command. Any failure leaves the database unchanged; a
    /// resent operation identity returns the operation it recorded.
    pub fn execute_blocking(
        &mut self,
        request: CommandRequest,
    ) -> Result<dpm_store::RecordedOperation, AppError> {
        let CommandRequest {
            actor,
            base_revision,
            base_lineage,
            operation_id,
            command,
        } = request;
        let preconditions = Preconditions {
            base_revision,
            base_lineage,
            operation_id,
        };
        self.commit_blocking(preconditions, actor, Intent::Command(command))
    }
    /// Apply a reviewed full proposal; the operation records only its entity-level difference.
    pub fn apply_plan_change_blocking(
        &mut self,
        request: PlanChangeRequest,
    ) -> Result<dpm_store::RecordedOperation, AppError> {
        let PlanChangeRequest {
            actor,
            base_revision,
            base_lineage,
            operation_id,
            plan: proposed,
            reason,
        } = request;
        let preconditions = Preconditions {
            base_revision,
            base_lineage,
            operation_id,
        };
        self.commit_blocking(
            preconditions,
            actor,
            Intent::PlanChange { proposed, reason },
        )
    }
    /// Build a command from live state, unless the supplied identity is already recorded and the
    /// request can no longer be rebuilt: a resend then answers `duplicate_operation` with the
    /// recorded operation instead of an error about state its own first attempt changed.
    pub fn build_command_blocking<T, E: From<AppError>>(
        &self,
        operation_id: Option<OperationId>,
        build: impl FnOnce(&Self) -> Result<T, E>,
    ) -> Result<T, E> {
        let built = build(self);
        #[cfg(feature = "sqlite")]
        return self.recorded_instead_blocking(built, operation_id);
        #[cfg(not(feature = "sqlite"))]
        {
            // Without a store no identity is ever recorded.
            let _ = operation_id;
            built
        }
    }
    /// Without a store there is nothing to write: every mutation is a read-only refusal.
    #[cfg(not(feature = "sqlite"))]
    fn commit_blocking(
        &mut self,
        _: Preconditions,
        _: dpm_model::ActorId,
        _: Intent,
    ) -> Result<dpm_store::RecordedOperation, AppError> {
        Err(AppError::ReadOnlyProject)
    }
}

/// Client identity and preconditions shared by every mutation; only a store reads them.
#[cfg_attr(not(feature = "sqlite"), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
struct Preconditions {
    base_revision: u64,
    base_lineage: Option<LineageId>,
    operation_id: Option<OperationId>,
}

/// What the client asked to change, kept in the form a resend is compared in.
#[cfg_attr(not(feature = "sqlite"), allow(dead_code))]
enum Intent {
    Command(Command),
    PlanChange { proposed: Box<Plan>, reason: String },
}

mod clock;
pub use clock::QueryClock;
mod query;
mod typed;
pub use typed::{NextRequest, Observed, StatusView, WorkDetail, WorkspaceRevision};
mod refresh;
mod watch;

#[cfg(feature = "sqlite")]
mod operation;
#[cfg(feature = "sqlite")]
mod store;

#[cfg(feature = "sqlite")]
mod recovery;
#[cfg(feature = "sqlite")]
pub use recovery::{restore_store_blocking, store_path_blocking, verify_store_blocking};

#[cfg(test)]
#[cfg(feature = "sqlite")]
mod tests;
