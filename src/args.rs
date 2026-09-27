use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[derive(Debug, Parser)]
#[command(name = "dpm", version, about = "Agent-first execution planning")]
pub(crate) struct Cli {
    /// Explicit SQLite path, overriding project discovery.
    #[arg(long, alias = "db", global = true, conflicts_with = "project")]
    pub(crate) database: Option<PathBuf>,

    /// Exact project directory containing .dpm/project.toml; init creates a project here.
    #[arg(long, global = true, conflicts_with = "database")]
    pub(crate) project: Option<PathBuf>,

    /// Emit machine-readable JSON where supported.
    #[arg(long, global = true)]
    pub(crate) json: bool,

    /// Reject mutations if the workspace revision has changed.
    #[arg(long, global = true)]
    pub(crate) base_revision: Option<u64>,

    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Commands {
    /// Preview or apply reviewed plan changes without replacing live state.
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },
    #[command(flatten)]
    Store(StoreCommand),
    /// Manage device-local bindings to existing workspaces.
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// Create .dpm/project.toml and an empty local database in the chosen directory.
    Init {
        #[arg(default_value = "DPM workspace")]
        name: String,
    },
    /// Load the prepared, execution-gated dpm self-host roadmap.
    Demo,
    /// Import a validated JSON plan into an uninitialized database.
    Import { file: PathBuf },
    /// Export the authoritative plan as JSON.
    Export,
    /// Validate a JSON plan without changing local state.
    Validate { file: PathBuf },
    /// Show high-level execution state and schedule risk.
    Status {
        #[arg(long)]
        no_simulation: bool,
    },
    /// Recommend executable work and explain the ranking.
    Next {
        #[arg(long = "capability")]
        capabilities: Vec<String>,
        /// Limit results to this project key's subtree; repeatable. Unlike --project, not a directory.
        #[arg(long = "project-key")]
        project_keys: Vec<String>,
        /// Limit results to work fitting these resource keys; repeatable.
        #[arg(long = "resource-key")]
        resource_keys: Vec<String>,
        #[arg(long, default_value_t = 5)]
        limit: usize,
        #[arg(long)]
        deterministic_only: bool,
    },
    /// Show one work item by human-readable key.
    Show { key: String },
    /// Explain why a work item exists in the current execution state.
    Explain { key: String },
    /// Approve a proposed contract with objective and acceptance criteria.
    Ratify {
        key: String,
        #[arg(long)]
        actor: String,
    },
    /// Return submitted work to its owner with an independent review.
    Reject {
        key: String,
        reason: String,
        #[arg(long)]
        actor: String,
    },
    #[command(flatten)]
    Ownership(OwnershipCommand),
    /// Start owned claimed work, recording the start event SS/SF successors wait for.
    Start {
        key: String,
        #[arg(long)]
        actor: String,
    },
    /// Mark work blocked with a concrete reason.
    Block {
        key: String,
        reason: String,
        #[arg(long)]
        actor: String,
    },
    /// Resume blocked work, retaining its owner when claimed.
    Unblock {
        key: String,
        #[arg(long)]
        actor: String,
    },
    /// Report owned task progress (0..100); 100% still requires submission and verification.
    Progress {
        key: String,
        percent: u8,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        actor: String,
    },
    /// Submit started work for verification once its FF/SF prerequisites are released.
    Submit {
        key: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        actor: String,
    },
    /// Verify submitted work, recording the finish event FS/FF successors wait for.
    Verify {
        key: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        actor: String,
    },
    /// Resolve an open decision as a human or service; a decision with options takes exactly one option key as outcome.
    Decide {
        key: String,
        outcome: String,
        #[arg(long)]
        actor: String,
    },
    /// Attach the current Git commit as an artifact to a work item.
    AttachGitHead {
        key: String,
        #[arg(long)]
        actor: String,
        /// Repository resource represented by the current checkout; defaults to the locator binding.
        #[arg(long)]
        resource: Option<String>,
    },
    /// Attach an Artifact JSON object through the shared command API.
    Artifact {
        key: String,
        file: PathBuf,
        #[arg(long)]
        actor: String,
    },
    /// Link work to an external tracker object; context only, never evidence or verification.
    LinkExternal {
        key: String,
        #[command(flatten)]
        identity: ExternalIdentityArgs,
        /// Display label; defaults to the recorded label or the identity.
        #[arg(long)]
        label: Option<String>,
        /// Display URL on the identity's instance, without credentials.
        #[arg(long)]
        url: Option<String>,
        /// tracks (the single local owner) or relates (context).
        #[arg(long, default_value = "tracks")]
        role: String,
        /// Reported external state (open, closed, merged); recorded only as an observation.
        #[arg(long)]
        observed: Option<String>,
        #[arg(long)]
        actor: String,
    },
    /// Remove a work item's link to an external tracker object.
    UnlinkExternal {
        key: String,
        #[command(flatten)]
        identity: ExternalIdentityArgs,
        #[arg(long)]
        actor: String,
    },
    /// Open the Ratatui operator console.
    Tui,
    /// Stop enforcing a soft dependency as a human or service; hard dependencies need plan review.
    WaiveDependency {
        /// Stable dependency identity, as shown by explain or export.
        dependency: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        actor: String,
    },
    /// Enforce a waived soft dependency again as a human or service.
    RestoreDependency {
        /// Stable dependency identity, as shown by explain or export.
        dependency: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        actor: String,
    },
    /// Re-base started work on a predecessor's current attempt after the relied-on one was rejected.
    RevalidateBasis {
        /// Successor whose provisional basis was invalidated.
        key: String,
        /// Stable identity of the provisional edge, as shown by explain.
        #[arg(long)]
        dependency: String,
        /// Predecessor attempt the reviewer checked; must be its current pending or verified one.
        #[arg(long)]
        attempt: u32,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        actor: String,
    },
}

/// Operation log and store-file commands, flattened into the top-level command list.
#[derive(Debug, Subcommand)]
pub(crate) enum StoreCommand {
    /// Read append-only semantic operations in chronological pages.
    History {
        #[arg(long, default_value_t = 0)]
        after_sequence: u64,
        #[arg(long, default_value_t = 100)]
        limit: u16,
    },
    /// Write a consistent, verified copy of the store, including all history, to a new file.
    Backup {
        /// New backup file; an existing file is never overwritten.
        #[arg(long)]
        to: PathBuf,
    },
    /// Restore a verified backup into a new store file, then verify it; never overwrites.
    Restore {
        /// Backup written by `dpm backup`.
        #[arg(long)]
        from: PathBuf,
        /// New store location; point a locator or binding at it explicitly afterwards.
        #[arg(long)]
        to: PathBuf,
    },
    /// Check pages, schema version, snapshot and history continuity without writing.
    VerifyStore {
        /// Store or backup file; defaults to the selected workspace's store.
        path: Option<PathBuf>,
    },
}

/// Commands that change who holds a task, flattened into the top-level command list.
#[derive(Debug, Subcommand)]
pub(crate) enum OwnershipCommand {
    /// Reserve a ready work item; a claim is not a start.
    Claim {
        key: String,
        #[arg(long)]
        actor: String,
    },
    /// Give up your own unstarted claim; the task returns to Planned without an owner.
    Release {
        key: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        actor: String,
    },
    /// Hand claimed, started or blocked work to another actor as a human or service; history,
    /// events, attempts, basis, progress and blocker stay with the work.
    Handoff {
        key: String,
        /// New owner as KIND:NAME.
        #[arg(long)]
        to: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        actor: String,
    },
}

/// Provider-scoped identity of an external object, independent of labels and URLs.
#[derive(Debug, clap::Args)]
pub(crate) struct ExternalIdentityArgs {
    /// Provider family: github, gitlab, forgejo, gitea, jira, linear or another name.
    #[arg(long)]
    pub(crate) provider: String,
    /// Hosted or self-hosted instance as host[:port], such as github.com.
    #[arg(long)]
    pub(crate) instance: String,
    /// Tenant, owner/repository or project namespace where the provider scopes identifiers.
    #[arg(long)]
    pub(crate) namespace: Option<String>,
    /// Object kind from the provider table (case, separators and one plural ignored): issue or
    /// pull_request (pr, mr, pulls) everywhere they exist; GitHub also discussion; GitLab also
    /// incident, task, test_case, ticket, objective, key_result, work_item and epic; any Jira or
    /// Linear issue type. Unlisted kinds are refused on GitHub, GitLab, Forgejo and Gitea.
    #[arg(long, default_value = "issue")]
    pub(crate) kind: String,
    /// Provider identifier: a number on forges (#42, !42 and 042 are 42), a PROJECT-N key on
    /// Jira and Linear (proj-06 is PROJ-6).
    #[arg(long = "id")]
    pub(crate) external_id: String,
}

#[derive(Debug, Subcommand)]
pub(crate) enum WorkspaceCommand {
    /// Bind an existing database by its workspace identity; use --database PATH.
    Register {
        /// Explicitly redirect a binding, or rebind a path held by another workspace identity.
        #[arg(long)]
        replace: bool,
    },
    /// List bindings with each store status and any identity sharing its path.
    List,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PlanCommand {
    /// Print the JSON Schema of the plan that export writes and plan diff / plan apply accept.
    Schema,
    /// Print a minimal valid proposal for this workspace while it has no projects or work.
    Template,
    /// Validate a candidate exported plan and inspect its semantic differences.
    Diff { file: PathBuf },
    /// Apply a reviewed candidate with a reason, preserving execution and evidence.
    Apply {
        file: PathBuf,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        actor: String,
    },
    /// Map a Microsoft Project XML (MSPDI) file onto a reviewed candidate; changes nothing.
    ImportMspdi {
        file: PathBuf,
        /// Existing project receiving the imported work.
        #[arg(long)]
        project_key: String,
        /// Prefix for keys of new work (PREFIX-UID); defaults to the project key. Required for a
        /// document without GUIDs (such as OmniPlan's), where it names the source.
        #[arg(long)]
        key_prefix: Option<String>,
        /// Opt-in: match tasks without a GUID to the one existing work item in the project with
        /// the same title path (titles from the project root down); ambiguity refuses the import.
        #[arg(long, value_name = "RULE", value_parser = crate::interchange::existing_match)]
        match_existing_by: Option<dpm_app::ExistingMatch>,
        /// Also write the candidate plan here for review and `plan apply`.
        #[arg(long)]
        candidate: Option<PathBuf>,
    },
    /// Write one project's work as the supported Microsoft Project XML subset.
    ExportMspdi {
        /// Project whose work is written.
        #[arg(long)]
        project_key: String,
        /// Write the document here instead of standard output.
        #[arg(long)]
        output: Option<PathBuf>,
    },
}
