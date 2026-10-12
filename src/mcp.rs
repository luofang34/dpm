//! `dpm mcp`: the agent tools over newline-delimited JSON-RPC on stdin/stdout, bound to one principal.
//!
//! Every tool calls the same application operation as its CLI command, so a tool's structured
//! content equals that command's `--json` envelope.

mod server;
mod tools;
mod transport;

pub(crate) use server::McpServer;
pub(crate) use transport::{TransportError, serve_blocking};

use crate::args::Preconditions;
use crate::error::CliError;
use dpm_app::Application;

/// Each tool call carries its own revision, lineage and operation identity, so the CLI's
/// process-wide ones are refused rather than silently ignored.
pub(crate) fn refuse_process_preconditions(preconditions: Preconditions) -> Result<(), CliError> {
    let Preconditions {
        base_revision,
        base_lineage,
        operation_id,
    } = preconditions;
    if base_revision.is_some() || base_lineage.is_some() || operation_id.is_some() {
        return Err(CliError::Input(
            "dpm mcp takes no --base-revision, --base-lineage or --operation-id; each tool call carries its own"
                .into(),
        ));
    }
    Ok(())
}

/// Serve one connection on stdin/stdout until the client closes it. The actor and the plan are read
/// first, so a bad principal or a workspace that cannot be loaded fails before any protocol message
/// rather than on the first tool.
pub(crate) fn serve_stdio_blocking(
    open: impl FnOnce() -> Result<Application, CliError>,
    actor: &str,
) -> Result<(), CliError> {
    let started = crate::app::actor(actor).and_then(|actor| {
        let application = open()?;
        application.plan_blocking()?;
        Ok((application, actor))
    });
    let (application, actor) = started.map_err(|error| CliError::McpStartup(Box::new(error)))?;
    let mut server = McpServer::new(application, actor);
    serve_blocking(
        &mut server,
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
