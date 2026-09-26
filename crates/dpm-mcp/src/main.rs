//! Stdio entry point for a principal-bound DPM MCP connection.
use clap::Parser;
use dpm_app::Application;
use dpm_mcp::{McpServer, serve_blocking};
use dpm_model::ActorId;
use std::{io, path::PathBuf};

#[derive(Parser)]
#[command(version, about)]
struct Args {
    #[arg(long)]
    db: PathBuf,
    #[arg(long)]
    actor: String,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let (kind, name) = args
        .actor
        .split_once(':')
        .ok_or("actor must be human:name, agent:name or service:name")?;
    if name.trim().is_empty() {
        return Err("actor name must not be empty".into());
    }
    let actor = match kind {
        "human" => ActorId::human(name),
        "agent" => ActorId::agent(name),
        "service" => ActorId::service(name),
        _ => return Err("unknown actor kind".into()),
    };
    let app = Application::open_blocking(args.db)?;
    app.plan_blocking()?;
    let mut server = McpServer::new(app, actor);
    serve_blocking(&mut server, io::stdin().lock(), io::stdout().lock())?;
    Ok(())
}
