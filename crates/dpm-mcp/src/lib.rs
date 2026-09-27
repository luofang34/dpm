//! Newline-framed stdio MCP tools backed by the shared DPM application service.

mod launch;
mod server;
mod tools;
mod transport;

pub use launch::{Launch, parse_actor};
pub use server::McpServer;
pub use transport::{TransportError, serve_blocking};
