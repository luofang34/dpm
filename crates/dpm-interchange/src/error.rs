use crate::{SourceChange, SourceLinkChange};
use dpm_model::WorkKind;
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
    /// Neither the document nor some of its tasks carry a GUID, and the import names no source.
    #[error(
        "MSPDI document has no project GUID and {count} task(s) without a GUID (first UID {first_uid}); pass an explicit key prefix that names this source: work identity is then derived from the target project, the key prefix and the task UID, so re-importing with the same prefix updates the same work and another prefix plans separate work"
    )]
    SourceScopeRequired {
        /// Imported tasks without a GUID.
        count: usize,
        /// First such task UID in document order.
        first_uid: i64,
    },
    /// Matching GUID-less tasks to existing work by title path is not unique.
    #[error(
        "matching existing work by title path is ambiguous, so nothing is matched: {}",
        ambiguities.join("; ")
    )]
    AmbiguousMatch {
        /// Every ambiguous task or path, with the local work keys involved.
        ambiguities: Vec<String>,
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
    /// A source task would change the kind of existing work, which reviewed plan changes forbid.
    #[error(
        "MSPDI task UID {uid} (GUID {}) would change {key} from {from:?} to {to:?}; the kind of existing work is fixed, so restore the source outline or milestone flag, or plan new work",
        guid.as_deref().unwrap_or("none")
    )]
    KindChange {
        /// Source task UID.
        uid: i64,
        /// Source task GUID, when present.
        guid: Option<String>,
        /// Existing work key.
        key: String,
        /// Local kind.
        from: WorkKind,
        /// Kind the source implies.
        to: WorkKind,
    },
    /// Review refused the candidate at work that a source task maps to.
    #[error("{change}; refused: {source}")]
    Refused {
        /// Source task and the changes it attempts.
        change: SourceChange,
        /// Review refusal naming only local work.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    /// Review refused the candidate at a dependency that a source link maps to, or that the
    /// candidate removes because the document omits the link.
    #[error("{change}; refused: {source}")]
    RefusedLink {
        /// Source link and the change it attempts.
        change: Box<SourceLinkChange>,
        /// Review refusal naming only the dependency identity.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
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
            Self::KeyCollision { .. }
            | Self::Unrepresentable { .. }
            | Self::KindChange { .. }
            | Self::AmbiguousMatch { .. }
            | Self::Refused { .. }
            | Self::RefusedLink { .. } => "invalid_command",
            Self::Xml { .. }
            | Self::NotMspdi { .. }
            | Self::MalformedProject { .. }
            | Self::MalformedTask { .. }
            | Self::DuplicateUid { .. }
            | Self::DuplicateIdentity { .. }
            | Self::SourceScopeRequired { .. } => "invalid_request",
        }
    }
}
