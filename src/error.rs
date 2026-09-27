use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum CliError {
    #[error(transparent)]
    Application(#[from] dpm_app::AppError),
    #[error(transparent)]
    Engine(#[from] dpm_engine::EngineError),
    #[error(transparent)]
    Validation(#[from] dpm_model::ValidationError),
    #[error(transparent)]
    Terminal(#[from] dpm_tui::TuiError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{action} at {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Input(String),
}

pub(crate) fn io_error(
    action: &'static str,
    path: impl Into<PathBuf>,
) -> impl FnOnce(std::io::Error) -> CliError {
    let path = path.into();
    move |source| CliError::Io {
        action,
        path,
        source,
    }
}

impl CliError {
    /// Machine envelope whose `error` object equals the MCP tool-error `structuredContent`.
    pub(crate) fn envelope(&self) -> serde_json::Value {
        serde_json::json!({ "error": dpm_app::ErrorResponse {
            api_version: dpm_app::API_VERSION,
            code: self.code(),
            message: self.to_string(),
            details: self.details(),
        }})
    }

    pub(crate) fn details(&self) -> Option<serde_json::Value> {
        match self {
            Self::Application(error) => error.details(),
            _ => None,
        }
    }

    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Application(error) => error.code(),
            Self::Io { .. } => "storage_error",
            Self::Engine(_) => "invalid_command",
            Self::Validation(_) => "invalid_plan",
            Self::Terminal(_) => "terminal_error",
            Self::Json(_) | Self::Input(_) => "invalid_request",
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests;
