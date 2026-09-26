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
    /// Claim a ready work item.
    Claim {
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
    /// Submit claimed work for verification.
    Submit {
        key: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, default_value = "agent:local")]
        actor: String,
    },
    /// Verify submitted work; successors can become ready immediately.
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
    /// Open the Ratatui operator console.
    Tui,
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
