//! Argument-parsing failures, reported in the same machine envelope as every other `--json` error.

use crate::output;
use clap::error::ErrorKind;
use std::{ffi::OsString, process::ExitCode};

/// Whether the caller asked for machine output, read from raw arguments because parsing failed.
pub(crate) fn json_requested(arguments: impl IntoIterator<Item = OsString>) -> bool {
    arguments
        .into_iter()
        .take_while(|argument| argument != "--")
        .any(|argument| argument == "--json")
}

/// The `--json` error envelope for a usage error, or `None` when clap's own text is the answer:
/// requested help or version output, or a human caller.
pub(crate) fn envelope(error: &clap::Error, json: bool) -> Option<serde_json::Value> {
    if !json
        || matches!(
            error.kind(),
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
        )
    {
        return None;
    }
    Some(serde_json::json!({ "error": dpm_app::ErrorResponse {
        api_version: dpm_app::API_VERSION,
        code: "invalid_request",
        message: error.render().to_string().trim().to_owned(),
        details: None,
    }}))
}

/// Print a parsing failure and choose the exit status clap would use.
pub(crate) fn report_blocking(error: &clap::Error, json: bool) -> ExitCode {
    let status = ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(2));
    let printed = match envelope(error, json) {
        Some(value) => output::json_blocking(&value).map_err(|e| e.to_string()),
        None => error.print().map_err(|e| e.to_string()),
    };
    if let Err(output_error) = printed {
        tracing::error!(%output_error, "could not write usage error");
    }
    status
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
