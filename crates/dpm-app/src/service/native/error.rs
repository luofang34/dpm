//! Typed refusals of the native boundary.

use super::protocol::{SourceChange, SourceIdentity};
use crate::AppError;
use dpm_model::{LineageId, WorkspaceId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

/// Why a native call was refused.
///
/// Refusals of the shared application keep their own stable codes; the boundary adds only the
/// codes that concern the boundary itself.
#[derive(Debug, Error)]
pub enum NativeError {
    /// The client offered no protocol version this build speaks.
    #[error("protocol {offered:?} is not supported; this build speaks {supported:?}")]
    UnsupportedProtocol {
        /// The versions the client offered.
        offered: Vec<u32>,
        /// The versions this build speaks.
        supported: Vec<u32>,
    },
    /// The request is not valid JSON of the request shape.
    #[error("malformed native request: {0}")]
    Malformed(#[source] serde_json::Error),
    /// The client is attached to another workspace than the source holds.
    #[error("attached to workspace {expected}, but the source holds workspace {actual}")]
    WorkspaceMismatch {
        /// The workspace the client expected.
        expected: WorkspaceId,
        /// The workspace the source holds.
        actual: WorkspaceId,
    },
    /// The client's lineage is not the one the source continues.
    #[error(
        "lineage conflict: the source continues {actual:?}, the client observed {expected:?}; \
         re-attach before reading"
    )]
    LineageMismatch {
        /// The lineage the client observed.
        expected: Option<LineageId>,
        /// The lineage the source continues.
        actual: Option<LineageId>,
    },
    /// The source a project locator selects is not the one this connection was opened on.
    #[error(
        "the selected source is not the one this connection opened ({reason}); reopen and re-attach"
    )]
    SourceChanged {
        /// What differs.
        reason: SourceChange,
        /// The source this connection reads.
        attached: SourceIdentity,
        /// The source the locator now selects.
        current: SourceIdentity,
    },
    /// The project kept changing identity or revision under a read, so its answer could not be
    /// tied to what it was anchored to. Run telemetry never causes this: it is only for the project
    /// history the answer itself depends on.
    #[error(
        "the project kept changing under the read: {attempts} attempts each saw a different history or revision; retry"
    )]
    Changing {
        /// Attempts made.
        attempts: u32,
    },
    /// The shared application refused the call.
    #[error(transparent)]
    App(Box<AppError>),
}

impl From<AppError> for NativeError {
    fn from(error: AppError) -> Self {
        Self::App(Box::new(error))
    }
}

/// A typed refusal on the wire, in the shape CLI and tool error objects use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeErrorBody {
    /// Application wire contract version.
    pub api_version: u32,
    /// Stable error category; clients must not parse the message.
    pub code: String,
    /// Human-readable context.
    pub message: String,
    /// Structured context for refusals a client can act on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl NativeError {
    /// The stable category of this refusal.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedProtocol { .. } => "unsupported_protocol",
            Self::Malformed(_) => "invalid_request",
            Self::WorkspaceMismatch { .. } => "workspace_mismatch",
            Self::LineageMismatch { .. } => "lineage_mismatch",
            Self::SourceChanged { .. } => "source_changed",
            Self::Changing { .. } => "workspace_changing",
            Self::App(error) => error.code(),
        }
    }

    /// The refusal as it crosses the boundary.
    #[must_use]
    pub fn body(&self) -> NativeErrorBody {
        if let Self::App(error) = self {
            let response = error.response();
            return NativeErrorBody {
                api_version: response.api_version,
                code: response.code.to_string(),
                message: response.message,
                details: response.details,
            };
        }
        let details = match self {
            Self::UnsupportedProtocol { offered, supported } => {
                Some(json!({"offered": offered, "supported": supported}))
            }
            Self::WorkspaceMismatch { expected, actual } => {
                Some(json!({"expected": expected, "actual": actual}))
            }
            Self::LineageMismatch { expected, actual } => {
                Some(json!({"expected": expected, "actual": actual}))
            }
            Self::SourceChanged {
                reason,
                attached,
                current,
            } => Some(json!({"reason": reason, "attached": attached, "current": current})),
            Self::Changing { attempts } => Some(json!({"attempts": attempts})),
            Self::Malformed(_) | Self::App(_) => None,
        };
        NativeErrorBody {
            api_version: crate::API_VERSION,
            code: self.code().to_string(),
            message: self.to_string(),
            details,
        }
    }
}
