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
    /// Read append-only semantic operations in chronological pages.
    History {
        #[arg(long, default_value_t = 0)]
        after_sequence: u64,
        #[arg(long, default_value_t = 100)]
        limit: u16,
    },
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
        #[arg(long, default_value = "human:local")]
        actor: String,
    },
    /// Return submitted work to its owner with an independent review.
    Reject {
        key: String,
        reason: String,
        #[arg(long, default_value = "human:local")]
        actor: String,
    },
    /// Reserve a ready work item; a claim is not a start.
    Claim {
        key: String,
        #[arg(long, default_value = "agent:local")]
        actor: String,
    },
    /// Start owned claimed work, recording the start event SS/SF successors wait for.
    Start {
        key: String,
        #[arg(long, default_value = "agent:local")]
        actor: String,
    },
    /// Mark work blocked with a concrete reason.
    Block {
        key: String,
        reason: String,
        #[arg(long, default_value = "human:local")]
        actor: String,
    },
    /// Resume blocked work, retaining its owner when claimed.
    Unblock {
        key: String,
        #[arg(long, default_value = "human:local")]
        actor: String,
    },
    /// Report owned task progress (0..100); 100% still requires submission and verification.
    Progress {
        key: String,
        percent: u8,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, default_value = "agent:local")]
        actor: String,
    },
    /// Submit started work for verification once its FF/SF prerequisites are released.
    Submit {
        key: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, default_value = "agent:local")]
        actor: String,
    },
    /// Verify submitted work, recording the finish event FS/FF successors wait for.
    Verify {
        key: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, default_value = "human:local")]
        actor: String,
    },
    /// Resolve an open human/organizational decision gate.
    Decide {
        key: String,
        outcome: String,
        #[arg(long, default_value = "human:local")]
        actor: String,
    },
    /// Attach the current Git commit as an artifact to a work item.
    AttachGitHead {
        key: String,
        #[arg(long, default_value = "agent:local")]
        actor: String,
        /// Repository resource represented by the current checkout; defaults to the locator binding.
        #[arg(long)]
        resource: Option<String>,
    },
    /// Attach an Artifact JSON object through the shared command API.
    Artifact {
        key: String,
        file: PathBuf,
        #[arg(long, default_value = "agent:local")]
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
        #[arg(long, default_value = "agent:local")]
        actor: String,
    },
    /// Remove a work item's link to an external tracker object.
    UnlinkExternal {
        key: String,
        #[command(flatten)]
        identity: ExternalIdentityArgs,
        #[arg(long, default_value = "agent:local")]
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
        #[arg(long, default_value = "human:local")]
        actor: String,
    },
    /// Enforce a waived soft dependency again as a human or service.
    RestoreDependency {
        /// Stable dependency identity, as shown by explain or export.
        dependency: String,
        #[arg(long)]
        reason: String,
        #[arg(long, default_value = "human:local")]
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
    /// Object kind: issue, pull_request or another name.
    #[arg(long, default_value = "issue")]
    pub(crate) kind: String,
    /// Stable provider identifier, such as an issue number.
    #[arg(long = "id")]
    pub(crate) external_id: String,
}

#[derive(Debug, Subcommand)]
pub(crate) enum WorkspaceCommand {
    /// Bind an existing database by its workspace identity; use --database PATH.
    Register {
        /// Explicitly redirect a binding that already points to another store.
        #[arg(long)]
        replace: bool,
    },
    /// List registered workspace identities and device-local store locations.
    List,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PlanCommand {
    /// Validate a candidate exported plan and inspect its semantic differences.
    Diff { file: PathBuf },
    /// Apply a reviewed candidate with a reason, preserving execution and evidence.
    Apply {
        file: PathBuf,
        #[arg(long)]
        reason: String,
        #[arg(long, default_value = "human:local")]
        actor: String,
    },
    /// Map a Microsoft Project XML (MSPDI) file onto a reviewed candidate; changes nothing.
    ImportMspdi {
        file: PathBuf,
        /// Existing project receiving the imported work.
        #[arg(long)]
        project_key: String,
        /// Prefix for keys of new work (PREFIX-UID); defaults to the project key.
        #[arg(long)]
        key_prefix: Option<String>,
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
