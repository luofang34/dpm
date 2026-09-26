use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[derive(Debug, Parser)]
#[command(name = "dpm", version, about = "Agent-first execution planning")]
pub(crate) struct Cli {
    /// SQLite database path. Defaults to .dpm/dpm.sqlite.
    #[arg(long, global = true)]
    pub(crate) database: Option<PathBuf>,

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
    /// Create an empty local workspace.
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
