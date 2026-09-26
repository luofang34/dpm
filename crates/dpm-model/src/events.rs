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
}

impl Release {
    /// Evaluate one constraint against an adapter-supplied clock reading.
    ///
    /// Positive lag delays release by elapsed time. Negative lag (lead) only shapes the schedule
    /// projection: execution still waits for the event itself, so it is treated as zero here.
    #[must_use]
    pub fn evaluate(event: Option<EventTime>, lag_hours: f64, now: DateTime<Utc>) -> Self {
        let Some(event) = event else {
            return Self::AwaitingEvent;
        };
        let delay = if lag_hours > 0.0 { lag_hours } else { 0.0 };
        match event {
            EventTime::Unrecorded if delay > 0.0 => Self::UnrecordedEventTime,
            EventTime::Unrecorded => Self::Released {
                at: EventTime::Unrecorded,
            },
            EventTime::Recorded(event_at) => {
                match lag_delta(delay).and_then(|d| event_at.checked_add_signed(d)) {
                    Some(opens_at) if opens_at <= now => Self::Released {
                        at: EventTime::Recorded(opens_at),
                    },
                    Some(opens_at) => Self::Elapsing { event_at, opens_at },
                    None => Self::LagOutOfRange { event_at },
                }
            }
        }
    }

    /// Release time when satisfied.
    #[must_use]
    pub fn released_at(self) -> Option<EventTime> {
        match self {
            Self::Released { at } => Some(at),
            _ => None,
        }
    }
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
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
