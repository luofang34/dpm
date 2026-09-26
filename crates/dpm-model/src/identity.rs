use serde::{Deserialize, Serialize};
use uuid::Uuid;
macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        #[doc = concat!("Stable UUID identifying a ", stringify!($name), ".")]
        pub struct $name(#[doc = "Underlying UUID."] pub Uuid);

        impl $name {
            #[must_use]
            /// Create a new identifier.
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

uuid_id!(WorkspaceId);
uuid_id!(ProjectId);
uuid_id!(WorkItemId);
uuid_id!(RequirementId);
uuid_id!(ArtifactId);
uuid_id!(DecisionId);
uuid_id!(RiskId);
uuid_id!(OperationId);
uuid_id!(ResourceId);
uuid_id!(ExternalReferenceId);
uuid_id!(DependencyId);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
/// Human-readable identifier, unique within its entity category.
pub struct Key(#[doc = "Underlying key text."] pub String);

impl Key {
    /// Create a new identifier.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
