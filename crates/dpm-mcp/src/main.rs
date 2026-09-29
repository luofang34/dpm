//! Stdio entry point for a principal-bound DPM MCP connection.
use clap::Parser;
use dpm_app::{QueryClock, open_workspace_blocking};
use dpm_mcp::{Launch, McpServer, serve_blocking};
use std::io;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Launch::parse();
    // A startup failure has no JSON-RPC channel yet; name the stable code and remedy, as the CLI does.
    let app = open_workspace_blocking(
        &std::env::current_dir()?,
        args.project.as_deref(),
        args.database.as_deref(),
    )
    .and_then(|app| app.plan_blocking().map(|_| app))
    .map(|app| app.with_query_clock(args.clock.map_or(QueryClock::System, QueryClock::Fixed)))
    .map_err(|error| format!("{}: {error}", error.code()))?;
    let mut server = McpServer::new(app, args.actor);
    serve_blocking(&mut server, io::stdin().lock(), io::stdout().lock())?;
    Ok(())
}
