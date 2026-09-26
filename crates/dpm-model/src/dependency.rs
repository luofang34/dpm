use crate::{ActorId, DependencyId, WorkItemId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
/// Temporal lower bound relating two activity endpoints.
pub enum DependencyKind {
    /// Successor start is bounded by predecessor finish plus lag.
    FinishStart,
    /// Successor start is bounded by predecessor start plus lag.
    StartStart,
    /// Successor finish is bounded by predecessor finish plus lag.
    FinishFinish,
    /// Successor finish is bounded by predecessor start plus lag.
    StartFinish,
}

impl DependencyKind {
    /// Stable two-letter relation name, independent of declaration order.
    #[must_use]
    pub fn abbreviation(self) -> &'static str {
        match self {
            Self::FinishStart => "FS",
            Self::StartStart => "SS",
            Self::FinishFinish => "FF",
            Self::StartFinish => "SF",
        }
    }

    /// Predecessor event the relation waits for: its start (SS, SF) or its finish (FS, FF).
    #[must_use]
    pub fn predecessor_endpoint(self) -> crate::Endpoint {
        match self {
            Self::FinishStart | Self::FinishFinish => crate::Endpoint::Finish,
            Self::StartStart | Self::StartFinish => crate::Endpoint::Start,
        }
    }

    /// Successor event the relation constrains: its start (FS, SS) or its finish (FF, SF).
    #[must_use]
    pub fn successor_endpoint(self) -> crate::Endpoint {
        match self {
            Self::FinishStart | Self::StartStart => crate::Endpoint::Start,
            Self::FinishFinish | Self::StartFinish => crate::Endpoint::Finish,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Whether an authorized reviewer may temporarily set a constraint aside.
pub enum DependencyPolicy {
    /// Always enforced; only a reviewed plan change can alter or remove it.
    #[default]
    Hard,
    /// Enforced unless a human or service records an explicit waiver.
    Soft,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Recorded decision to stop enforcing a soft constraint.
#[serde(deny_unknown_fields)]
pub struct DependencyWaiver {
    /// Human or service principal accountable for the waiver.
    pub actor: ActorId,
    /// Caller-supplied UTC waiver time.
    pub at: chrono::DateTime<chrono::Utc>,
    /// Non-empty justification.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Independently addressable temporal constraint between two work items.
#[serde(from = "DependencyRecord")]
pub struct Dependency {
    /// Stable edge identity; unique within the plan.
    pub id: DependencyId,
    /// Activity providing the constrained start or finish.
    pub predecessor: WorkItemId,
    /// Activity whose start or finish has the lower bound.
    pub successor: WorkItemId,
    /// Domain category of this value.
    pub kind: DependencyKind,
    /// Positive values add delay; negative values are lead time.
    pub lag_hours: f64,
    /// Whether the constraint may be waived.
    pub policy: DependencyPolicy,
    /// Optional author explanation of why the constraint and its policy exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    /// Present while a soft constraint is set aside; changed only by waive/restore commands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiver: Option<DependencyWaiver>,
}

/// Serialized form accepted on input; plans that predate edge identity omit `id` and `policy`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencyRecord {
    #[serde(default)]
    id: Option<DependencyId>,
    predecessor: WorkItemId,
    successor: WorkItemId,
    kind: DependencyKind,
    lag_hours: f64,
    #[serde(default)]
    policy: DependencyPolicy,
    #[serde(default)]
    rationale: Option<String>,
    #[serde(default)]
    waiver: Option<DependencyWaiver>,
}

impl From<DependencyRecord> for Dependency {
    fn from(record: DependencyRecord) -> Self {
        Self {
            id: record.id.unwrap_or_else(|| {
                Dependency::derived_id(record.predecessor, record.successor, record.kind)
            }),
            predecessor: record.predecessor,
            successor: record.successor,
            kind: record.kind,
            lag_hours: record.lag_hours,
            policy: record.policy,
            rationale: record.rationale,
            waiver: record.waiver,
        }
    }
}

const FNV_OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const FNV_PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
const IDENTITY_DOMAIN: &[u8] = b"dpm.dependency.v1\0";

/// MurmurHash3's 64-bit finalizer: a bijection in which every input bit affects every output bit.
fn avalanche(mut value: u64) -> u64 {
    value ^= value >> 33;
    value = value.wrapping_mul(0xff51_afd7_ed55_8ccd);
    value ^= value >> 33;
    value = value.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    value ^ (value >> 33)
}

impl Dependency {
    /// Deterministic identity for an edge serialized without one.
    ///
    /// Every load of the same stored plan, and every store replaying its operation log, must agree
    /// on the identity, so it depends only on the ordered endpoints and relation. Validation allows
    /// one relation of each kind per ordered pair; a hash collision would surface as a
    /// duplicate-identity validation error rather than merging two edges.
    #[must_use]
    pub fn derived_id(
        predecessor: WorkItemId,
        successor: WorkItemId,
        kind: DependencyKind,
    ) -> DependencyId {
        let mut hash = FNV_OFFSET;
        let bytes = IDENTITY_DOMAIN
            .iter()
            .chain(predecessor.0.as_bytes())
            .chain(successor.0.as_bytes())
            .chain(kind.abbreviation().as_bytes());
        for byte in bytes {
            hash ^= u128::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        // FNV alone leaves inputs that differ only near the end sharing most high bits; this
        // bijective cross-mix of both halves spreads every input byte over the whole identity.
        let (high, low) = ((hash >> 64) as u64, hash as u64);
        let high = avalanche(high ^ low.rotate_left(29));
        let low = avalanche(low ^ high);
        let high = avalanche(high ^ low);
        let mixed = (u128::from(high) << 64) | u128::from(low);
        DependencyId(uuid::Builder::from_custom_bytes(mixed.to_be_bytes()).into_uuid())
    }

    /// Hard constraint with a derived identity and no rationale.
    #[must_use]
    pub fn new(
        predecessor: WorkItemId,
        successor: WorkItemId,
        kind: DependencyKind,
        lag_hours: f64,
    ) -> Self {
        Self {
            id: Self::derived_id(predecessor, successor, kind),
            predecessor,
            successor,
            kind,
            lag_hours,
            policy: DependencyPolicy::Hard,
            rationale: None,
            waiver: None,
        }
    }

    /// Whether a recorded waiver currently sets this constraint aside.
    #[must_use]
    pub fn is_waived(&self) -> bool {
        self.waiver.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
/// Contextual relationship between work items; never an execution or schedule constraint.
pub enum WorkLinkKind {
    /// The items share context.
    RelatesTo,
    /// The source repeats the target's scope.
    Duplicates,
    /// The source was derived from the target.
    DerivedFrom,
    /// The source replaces the target's scope.
    Supersedes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Typed non-gating link, identified by its kind and unordered endpoint pair.
#[serde(deny_unknown_fields)]
pub struct WorkLink {
    /// Meaning of the relationship.
    pub kind: WorkLinkKind,
    /// Work item making the statement.
    pub source: WorkItemId,
    /// Work item the statement refers to.
    pub target: WorkItemId,
    /// Optional non-empty explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
