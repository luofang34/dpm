use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
/// Principal category recorded in an audit operation.
pub enum ActorKind {
    /// A person.
    Human,
    /// An automated worker.
    Agent,
    /// An external service or CI verifier.
    Service,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Local principal identity; kind and name jointly distinguish actors.
#[serde(deny_unknown_fields)]
pub struct ActorId {
    /// Domain category of this value.
    pub kind: ActorKind,
    /// Non-empty local name.
    pub name: String,
}

impl ActorId {
    /// Create a human principal.
    pub fn human(name: impl Into<String>) -> Self {
        Self {
            kind: ActorKind::Human,
            name: name.into(),
        }
    }

    /// Create an agent principal.
    pub fn agent(name: impl Into<String>) -> Self {
        Self {
            kind: ActorKind::Agent,
            name: name.into(),
        }
    }

    /// Create a service principal.
    pub fn service(name: impl Into<String>) -> Self {
        Self {
            kind: ActorKind::Service,
            name: name.into(),
        }
    }
}

impl std::fmt::Display for ActorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            ActorKind::Human => "human",
            ActorKind::Agent => "agent",
            ActorKind::Service => "service",
        };
        write!(f, "{kind}:{}", self.name)
    }
}

/// A principal spelled other than `human:NAME`, `agent:NAME` or `service:NAME`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ActorParseError {
    /// No `KIND:` prefix.
    #[error("actor {0:?} must be human:NAME, agent:NAME, or service:NAME")]
    MissingKind(String),
    /// The kind is not one of the three principal categories.
    #[error("unknown actor kind {0}")]
    UnknownKind(String),
    /// The name after the kind is blank.
    #[error("actor name must not be empty")]
    EmptyName,
}

impl std::str::FromStr for ActorId {
    type Err = ActorParseError;

    /// Parse the `KIND:NAME` spelling every adapter accepts, so CLI flags and tool arguments name
    /// the same principal.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (kind, name) = value
            .split_once(':')
            .ok_or_else(|| ActorParseError::MissingKind(value.into()))?;
        if name.trim().is_empty() {
            return Err(ActorParseError::EmptyName);
        }
        let kind = match kind {
            "human" => ActorKind::Human,
            "agent" => ActorKind::Agent,
            "service" => ActorKind::Service,
            _ => return Err(ActorParseError::UnknownKind(kind.into())),
        };
        Ok(Self {
            kind,
            name: name.into(),
        })
    }
}
