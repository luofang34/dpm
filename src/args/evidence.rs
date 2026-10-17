//! Evidence commands that read local input before recording, flattened into the top-level list.

use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub(crate) enum EvidenceCommand {
    /// Attach the current Git commit as an artifact to a work item.
    AttachGitHead {
        key: String,
        #[arg(long)]
        actor: String,
        /// Repository asset represented by the current checkout; defaults to the locator binding.
        #[arg(long)]
        asset: Option<String>,
    },
    /// Attach an Artifact JSON object through the shared command API. Its URI references where the
    /// evidence lives (a commit, CI run, document or fingerprinted file); content and copies in
    /// .dpm are refused.
    Artifact {
        key: String,
        file: PathBuf,
        #[arg(long)]
        actor: String,
    },
    /// Attach a local file as evidence without copying it: its location, SHA-256 digest and size
    /// are recorded so the reference can be checked later. Files inside .dpm are refused.
    AttachFile {
        key: String,
        path: PathBuf,
        /// Artifact kind, such as TestResult, Build, Photo, Datasheet, Quote or File.
        #[arg(long)]
        kind: String,
        /// What the file shows.
        #[arg(long)]
        label: String,
        /// Result a TestResult or Build states: passed, failed or partial.
        #[arg(long)]
        outcome: Option<String>,
        /// Further attribute as KEY=VALUE, such as command=... or commit=...; repeatable.
        #[arg(long = "meta", value_name = "KEY=VALUE")]
        metadata: Vec<String>,
        #[arg(long)]
        actor: String,
    },
}
