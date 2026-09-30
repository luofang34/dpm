use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

/// Times recorded by a task's own lifecycle commands; never supplied by plan authors.
///
/// Each field is written only by the command that performs the transition, using that
/// operation's timestamp. Work that reached a state before these facts existed has no time here,
/// and gates treat that event as having occurred at an unknown time.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionEvents {
    /// When the owner started executing the task, as distinct from reserving it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// When the current submission was made; a rejection clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<DateTime<Utc>>,
    /// When an independent verifier accepted the result; the task's finish event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<DateTime<Utc>>,
    /// The task started before start times were recorded. Blocking such work sets it, because a
    /// `Blocked` lifecycle alone cannot say whether the work had started.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub start_unrecorded: bool,
}

impl ExecutionEvents {
    /// Whether no event time has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// When an event that has occurred happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventTime {
    /// Time recorded by the command that caused the event.
    Recorded(DateTime<Utc>),
    /// The event occurred before event times were recorded; its time is unknown.
    Unrecorded,
}

impl EventTime {
    /// The later of two event times; an unknown time is never assumed to be earlier.
    #[must_use]
    pub fn latest(self, other: Self) -> Self {
        match (self, other) {
            (Self::Recorded(a), Self::Recorded(b)) => Self::Recorded(a.max(b)),
            _ => Self::Unrecorded,
        }
    }

    /// Recorded time, if known.
    #[must_use]
    pub fn recorded(self) -> Option<DateTime<Utc>> {
        match self {
            Self::Recorded(at) => Some(at),
            Self::Unrecorded => None,
        }
    }
}

/// One end of an activity; zero-duration milestones start and finish at the same event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Endpoint {
    /// Task start event, or a milestone's reach event.
    Start,
    /// Task verification event, or a milestone's reach event.
    Finish,
}

/// State of a constraint that waits for an event plus elapsed lag, at one clock reading.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Release {
    /// The required event has not occurred.
    AwaitingEvent,
    /// The event occurred and positive lag is still elapsing.
    Elapsing {
        /// Recorded time of the required event.
        event_at: DateTime<Utc>,
        /// Earliest clock reading at which the constraint is satisfied.
        opens_at: DateTime<Utc>,
    },
    /// Positive lag must elapse from an event whose time was never recorded.
    UnrecordedEventTime,
    /// The lag cannot be added to the event time within the supported calendar range.
    LagOutOfRange {
        /// Recorded time of the required event.
        event_at: DateTime<Utc>,
    },
    /// The constraint is satisfied.
    Released {
        /// When it became satisfied: the event time plus any positive lag.
        at: EventTime,
    },
    /// The predecessor was excluded by a choice and the successor is an active-branch join, so
    /// the constraint is an absent branch rather than a satisfied prerequisite.
    SkippedBranch {
        /// When the excluding choice was made.
        at: EventTime,
    },
    /// The predecessor was excluded by a choice; an ordinary constraint from it never releases.
    NotSelected,
}

impl Release {
    /// Evaluate one constraint against an adapter-supplied clock reading.
    ///
    /// Positive lag delays release by elapsed time. Negative lag (lead) only shapes the schedule
    /// projection: execution still waits for the event itself, so it is treated as zero here.
    #[must_use]
    pub fn evaluate(event: Option<EventTime>, lag_hours: f64, now: DateTime<Utc>) -> Self {
        Self::evaluate_with(event, lag_hours, now, hours_after)
    }

    /// Evaluate one constraint whose positive lag ends at `lag_end(event_at, hours)`, such as
    /// working hours of a calendar; `None` from it means the lag is out of range.
    #[must_use]
    pub fn evaluate_with(
        event: Option<EventTime>,
        lag_hours: f64,
        now: DateTime<Utc>,
        lag_end: impl FnOnce(DateTime<Utc>, f64) -> Option<DateTime<Utc>>,
    ) -> Self {
        let Some(event) = event else {
            return Self::AwaitingEvent;
        };
        let delay = if lag_hours > 0.0 { lag_hours } else { 0.0 };
        match event {
            EventTime::Unrecorded if delay > 0.0 => Self::UnrecordedEventTime,
            EventTime::Unrecorded => Self::Released {
                at: EventTime::Unrecorded,
            },
            EventTime::Recorded(event_at) => match lag_end(event_at, delay) {
                Some(opens_at) if opens_at <= now => Self::Released {
                    at: EventTime::Recorded(opens_at),
                },
                Some(opens_at) => Self::Elapsing { event_at, opens_at },
                None => Self::LagOutOfRange { event_at },
            },
        }
    }

    /// Release time when satisfied.
    #[must_use]
    pub fn released_at(self) -> Option<EventTime> {
        match self {
            Self::Released { at } | Self::SkippedBranch { at } => Some(at),
            _ => None,
        }
    }
}

/// The instant `hours` elapsed hours after `at`; `None` outside chrono's range.
pub(crate) fn hours_after(at: DateTime<Utc>, hours: f64) -> Option<DateTime<Utc>> {
    lag_delta(hours).and_then(|delta| at.checked_add_signed(delta))
}

/// Millisecond-resolution elapsed lag; `None` when it exceeds the representable range.
fn lag_delta(hours: f64) -> Option<TimeDelta> {
    let millis = (hours * 3_600_000.0).round();
    // chrono's range is far below 2^53 ms, so this bound also keeps the cast exact.
    if !millis.is_finite() || millis.abs() > 9.0e15 {
        return None;
    }
    TimeDelta::try_milliseconds(millis as i64)
}

#[cfg(test)]
mod tests;
