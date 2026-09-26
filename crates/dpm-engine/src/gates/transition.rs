use dpm_model::{DependencyKind, DependencyPolicy, Endpoint, Release, WorkStatus};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Task lifecycle transition whose relation gates are evaluated.
///
/// | Transition | From        | Relations  | Decisions |
/// |------------|-------------|------------|-----------|
/// | `Claim`    | `Planned`   | FS, SS     | yes       |
/// | `Start`    | `Claimed`   | FS, SS     | yes       |
/// | `Submit`   | `InProgress`| FF, SF     | no        |
/// | `Verify`   | `Submitted` | all four   | yes       |
///
/// A claim reserves only work that could start now, so it shares the start gates. Submission is the
/// owner's finish claim and verification the accepted finish: both must respect FF/SF. Verification
/// also re-checks FS/SS and decisions, so a restored waiver or gate applies again before acceptance.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    /// Reserve planned work for an owner.
    #[default]
    Claim,
    /// Begin executing claimed work; records the start event.
    Start,
    /// Request independent verification; records the submission event.
    Submit,
    /// Accept the submitted result; records the finish event.
    Verify,
}

impl Transition {
    /// Every transition, in lifecycle order.
    pub const ALL: [Self; 4] = [Self::Claim, Self::Start, Self::Submit, Self::Verify];

    /// Lifecycle state the transition leaves.
    #[must_use]
    pub fn lifecycle(self) -> WorkStatus {
        match self {
            Self::Claim => WorkStatus::Planned,
            Self::Start => WorkStatus::Claimed,
            Self::Submit => WorkStatus::InProgress,
            Self::Verify => WorkStatus::Submitted,
        }
    }

    /// Whether a relation constrains this transition of its successor.
    #[must_use]
    pub fn governs(self, kind: DependencyKind) -> bool {
        match self {
            Self::Claim | Self::Start => kind.successor_endpoint() == Endpoint::Start,
            Self::Submit => kind.successor_endpoint() == Endpoint::Finish,
            Self::Verify => true,
        }
    }

    /// Whether open decisions on the work or its containers prevent this transition.
    #[must_use]
    pub fn checks_decisions(self) -> bool {
        !matches!(self, Self::Submit)
    }
}

impl fmt::Display for Transition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Claim => "claim",
            Self::Start => "start",
            Self::Submit => "submit",
            Self::Verify => "verify",
        })
    }
}

pub(super) fn describe_release(
    key: &str,
    requires: Endpoint,
    release: Release,
    edge: &str,
    policy: DependencyPolicy,
) -> String {
    let event = match requires {
        Endpoint::Start => "start",
        Endpoint::Finish => "verified finish",
    };
    match release {
        Release::AwaitingEvent => format!("awaits the {event} of {key} ({edge})"),
        Release::Elapsing { event_at, opens_at } => {
            format!("{key} {event} recorded at {event_at}; lag elapses at {opens_at} ({edge})")
        }
        Release::UnrecordedEventTime => {
            let remedy = match policy {
                DependencyPolicy::Soft => "a human or service may waive this Soft edge",
                DependencyPolicy::Hard => {
                    "a reviewed plan change may alter the lag while this work is unstarted"
                }
            };
            format!(
                "the {event} time of {key} was not recorded, so its positive lag cannot be shown to have elapsed ({edge}); {remedy}"
            )
        }
        Release::LagOutOfRange { event_at } => format!(
            "{key} {event} recorded at {event_at}, but the lag exceeds the supported time range ({edge})"
        ),
        Release::Released { .. } => format!("{key} {event} released ({edge})"),
    }
}
