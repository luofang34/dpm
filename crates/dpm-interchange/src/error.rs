use thiserror::Error;

/// Interchange failure; every variant leaves the workspace untouched because imports only build
/// a candidate for review.
#[derive(Debug, Error)]
pub enum InterchangeError {
    /// The document is not well-formed XML.
    #[error("MSPDI document is not well-formed XML: {source}")]
    Xml {
        /// Parser diagnostic with position.
        #[source]
        source: roxmltree::Error,
    },
    /// The root element is not a Microsoft Project XML project.
    #[error(
        "document root is {found}; expected <Project> in the http://schemas.microsoft.com/project namespace"
    )]
    NotMspdi {
        /// Qualified name of the root element found.
        found: String,
    },
    /// A project-level element violates the MSPDI structure DPM relies on.
    #[error("MSPDI project: {reason}")]
    MalformedProject {
        /// Violated structural rule.
        reason: String,
    },
    /// A task or link element violates the MSPDI structure DPM relies on.
    #[error("MSPDI task at position {position}: {reason}")]
    MalformedTask {
        /// One-based position of the task element in the document.
        position: usize,
        /// Violated structural rule.
        reason: String,
    },
    /// Two task elements share a unique identifier.
    #[error("MSPDI task UID {uid} appears more than once")]
    DuplicateUid {
        /// Repeated source UID.
        uid: i64,
    },
    /// Two task elements resolve to the same local work identity.
    #[error(
        "MSPDI tasks UID {first} and UID {second} resolve to the same work identity {identity}"
    )]
    DuplicateIdentity {
        /// First task UID.
        first: i64,
        /// Second task UID.
        second: i64,
        /// Shared identity.
        identity: String,
    },
    /// The requested project key does not exist in the workspace.
    #[error("unknown project key {key}")]
    UnknownProject {
        /// Requested project key.
        key: String,
    },
    /// A generated key is already used by other work.
    #[error("key {key} for MSPDI task UID {uid} is already used; choose another key prefix")]
    KeyCollision {
        /// Generated work key.
        key: String,
        /// Source task UID.
        uid: i64,
    },
    /// A value cannot be written as MSPDI.
    #[error("work {key} cannot be exported as MSPDI: {reason}")]
    Unrepresentable {
        /// Work key.
        key: String,
        /// Reason the value has no MSPDI form.
        reason: String,
    },
}

impl InterchangeError {
    /// Stable machine category shared by the CLI and agent adapters.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownProject { .. } => "not_found",
            Self::KeyCollision { .. } | Self::Unrepresentable { .. } => "invalid_command",
            Self::Xml { .. }
            | Self::NotMspdi { .. }
            | Self::MalformedProject { .. }
            | Self::MalformedTask { .. }
            | Self::DuplicateUid { .. }
            | Self::DuplicateIdentity { .. } => "invalid_request",
        }
    }
}
