use serde::{Deserialize, Serialize};
use uuid::Uuid;
macro_rules! uuid_id {
    ($name:ident) => {
        uuid_id!($name, new_v4);
    };
    ($name:ident, $mint:ident) => {
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
                Self(Uuid::$mint())
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

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(text).map(Self)
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
// Time-ordered identities sort by creation, so an operation log and its lineages index well.
uuid_id!(OperationId, now_v7);
uuid_id!(LineageId, now_v7);
uuid_id!(AssetId);
uuid_id!(ExternalReferenceId);
uuid_id!(DependencyId);

impl OperationId {
    /// Whether this is a version 7 UUID, the only form a client may supply as an idempotency key.
    #[must_use]
    pub fn is_time_ordered(&self) -> bool {
        self.0.get_version() == Some(uuid::Version::SortRand)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
/// Human-readable identifier, unique within its entity category.
pub struct Key(#[doc = "Underlying key text."] pub String);

impl Key {
    /// Create a new identifier.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Order keys as people number them: runs of ASCII digits compare by value, so `OP-2` sorts
    /// before `OP-10`, and everything else compares by character.
    ///
    /// Equal values with different leading zeros (`T-7`, `T-07`) and any remaining tie fall back
    /// to plain text order, so the order is total and consistent with equality.
    #[must_use]
    pub fn natural_cmp(&self, other: &Self) -> std::cmp::Ordering {
        let (mut left, mut right) = (self.0.as_str(), other.0.as_str());
        loop {
            let (a, rest_a) = next_run(left);
            let (b, rest_b) = next_run(right);
            let order = match (a, b) {
                (None, None) => return self.0.cmp(&other.0),
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(a), Some(b)) => compare_runs(a, b),
            };
            if order.is_ne() {
                return order;
            }
            (left, right) = (rest_a, rest_b);
        }
    }
}

/// The leading run of `text` (all ASCII digits or no ASCII digits) and the text after it.
fn next_run(text: &str) -> (Option<&str>, &str) {
    let digits = text.starts_with(|c: char| c.is_ascii_digit());
    let end = text
        .find(|c: char| c.is_ascii_digit() != digits)
        .unwrap_or(text.len());
    if end == 0 {
        return (None, text);
    }
    let (run, rest) = text.split_at(end);
    (Some(run), rest)
}

fn compare_runs(a: &str, b: &str) -> std::cmp::Ordering {
    let numeric = |run: &str| run.starts_with(|c: char| c.is_ascii_digit());
    if numeric(a) && numeric(b) {
        let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
        a.len().cmp(&b.len()).then_with(|| a.cmp(b))
    } else {
        a.cmp(b)
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests;
