use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
