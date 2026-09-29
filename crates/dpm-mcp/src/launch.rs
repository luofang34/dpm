//! Process arguments: one configured principal and one project selection per connection.

use clap::Parser;
use dpm_model::ActorId;
use std::path::PathBuf;

/// Options of the `dpm-mcp` stdio process.
#[derive(Debug, Parser)]
#[command(
    name = "dpm-mcp",
    version,
    about = "Serve DPM agent tools over newline-delimited JSON-RPC on stdin/stdout",
    after_help = "Without --database or --project, the project is discovered from the current directory \
                  exactly as the dpm CLI discovers it."
)]
pub struct Launch {
    /// SQLite store to serve (alias --db); skips project discovery.
    #[arg(
        long = "database",
        alias = "db",
        value_name = "PATH",
        conflicts_with = "project"
    )]
    pub database: Option<PathBuf>,
    /// Exact project directory containing .dpm/project.toml; skips discovery from the current directory.
    #[arg(long, value_name = "DIR", conflicts_with = "database")]
    pub project: Option<PathBuf>,
    /// Principal recorded on every mutation: human:NAME, agent:NAME or service:NAME.
    #[arg(long, value_name = "KIND:NAME", value_parser = parse_actor)]
    pub actor: ActorId,
    /// Evaluate every query at this RFC 3339 instant instead of the system clock, as the CLI's
    /// `--clock` does; mutations still record the time they commit.
    #[arg(long, value_name = "RFC3339")]
    pub clock: Option<chrono::DateTime<chrono::Utc>>,
}

/// Parse `KIND:NAME`; the kind decides which independence rules apply to the principal.
pub fn parse_actor(value: &str) -> Result<ActorId, String> {
    let (kind, name) = value
        .split_once(':')
        .ok_or("actor must be human:NAME, agent:NAME or service:NAME")?;
    if name.trim().is_empty() {
        return Err("actor name must not be empty".into());
    }
    match kind {
        "human" => Ok(ActorId::human(name)),
        "agent" => Ok(ActorId::agent(name)),
        "service" => Ok(ActorId::service(name)),
        _ => Err(format!(
            "unknown actor kind {kind}; use human, agent or service"
        )),
    }
}

#[cfg(test)]
mod tests;
