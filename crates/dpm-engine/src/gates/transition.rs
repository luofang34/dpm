use dpm_model::{Applicability, DependencyKind, DependencyPolicy, Endpoint, Release, WorkStatus};
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

    /// Whether this transition is gated as the successor's start, where a provisional edge may
    /// accept a pending predecessor attempt.
    #[must_use]
    pub fn starts(self) -> bool {
        matches!(self, Self::Claim | Self::Start)
    }

    /// Whether a basis on a rejected predecessor attempt prevents this transition: finishing work
    /// built on a rejected result would hide the rejection.
    #[must_use]
    pub fn checks_basis(self) -> bool {
        matches!(self, Self::Submit | Self::Verify)
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
                    "a Hard edge cannot be waived; a reviewed plan change may alter its lag only while the successor is unstarted, and otherwise the gate stays closed"
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
        Release::SkippedBranch { .. } => {
            format!(
                "{key} was not selected; this active-branch join treats it as an absent branch ({edge})"
            )
        }
        Release::NotSelected => format!(
            "{key} was not selected, and an ordinary dependency on skipped work never releases ({edge}); declare an active-branch join or change the plan"
        ),
    }
}

pub(super) fn describe_applicability(applicability: &Applicability) -> String {
    match applicability {
        Applicability::Applicable => "work is applicable".into(),
        Applicability::Undecided { decision, option } => {
            format!("work applies only if {decision} selects {option}, and it is undecided")
        }
        Applicability::NotSelected {
            decision,
            option,
            selected,
        } => format!(
            "work applies only if {decision} selects {option}, but it selected {selected}; it cannot be claimed or completed"
        ),
        Applicability::AwaitingChoice { predecessor } => format!(
            "prerequisite {predecessor} depends on an undecided choice, so this work is not yet committed"
        ),
        Applicability::Stranded { predecessor, .. } => format!(
            "prerequisite {predecessor} was not selected or can never proceed, and an ordinary dependency never releases from it; only a reviewed plan change that rewires the dependency, or a waiver of a Soft dependency, lets this work proceed"
        ),
        Applicability::EmptyJoin => "every branch into this join was not selected, and the join does not permit an empty result".into(),
    }
}
