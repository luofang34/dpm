use crate::{McpServer, server::error};
use serde_json::Value;
use std::io::{BufRead, Write};
use thiserror::Error;

/// Stdio transport failure.
#[derive(Debug, Error)]
pub enum TransportError {
    /// Reading/writing framed messages failed.
    #[error("MCP stdio: {0}")]
    Io(#[from] std::io::Error),
    /// Encoding a response failed.
    #[error("MCP response encoding: {0}")]
    Json(#[from] serde_json::Error),
}

/// Serve newline-delimited JSON-RPC until EOF. Stdout contains protocol messages only.
pub fn serve_blocking(
    server: &mut McpServer,
    mut input: impl BufRead,
    mut output: impl Write,
) -> Result<(), TransportError> {
    const MAX_FRAME: u64 = 8 * 1024 * 1024;
    loop {
        let mut line = String::new();
        let count = std::io::Read::take(&mut input, MAX_FRAME + 1).read_line(&mut line)?;
        if count == 0 {
            break;
        }
        let response = if count as u64 > MAX_FRAME {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "MCP frame exceeds 8 MiB",
            )
            .into());
        } else {
            match serde_json::from_str::<Value>(&line) {
                Ok(request) => server.handle_blocking(request),
                Err(_) => Some(error(Value::Null, -32700, "Parse error")),
            }
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}
