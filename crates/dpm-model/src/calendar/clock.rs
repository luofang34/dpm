//! Local times of day written `HH:MM`, and working periods between two of them.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// Minutes after local midnight, from `00:00` to `24:00` inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClockTime(u16);

/// Minutes in a day; `24:00` ends a period at the next midnight.
const DAY_MINUTES: u16 = 24 * 60;

impl ClockTime {
    /// A whole hour, clamped to `24:00`.
    #[must_use]
    pub const fn hours(hours: u16) -> Self {
        let minutes = hours.saturating_mul(60);
        Self(if minutes > DAY_MINUTES {
            DAY_MINUTES
        } else {
            minutes
        })
    }

    /// Time from minutes after midnight, or `None` beyond `24:00`.
    #[must_use]
    pub fn from_minutes(minutes: u16) -> Option<Self> {
        (minutes <= DAY_MINUTES).then_some(Self(minutes))
    }

    /// Minutes after midnight.
    #[must_use]
    pub fn minutes(self) -> u16 {
        self.0
    }
}

impl std::fmt::Display for ClockTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}", self.0 / 60, self.0 % 60)
    }
}

impl std::str::FromStr for ClockTime {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || format!("time {text:?} must be HH:MM between 00:00 and 24:00");
        let (hours, minutes) = text.split_once(':').ok_or_else(invalid)?;
        if hours.len() != 2 || minutes.len() != 2 {
            return Err(invalid());
        }
        let hours: u16 = hours.parse().map_err(|_| invalid())?;
        let minutes: u16 = minutes.parse().map_err(|_| invalid())?;
        if minutes >= 60 {
            return Err(invalid());
        }
        Self::from_minutes(hours.saturating_mul(60).saturating_add(minutes)).ok_or_else(invalid)
    }
}

impl Serialize for ClockTime {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

struct ClockVisitor;

impl de::Visitor<'_> for ClockVisitor {
    type Value = ClockTime;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a local time HH:MM")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<ClockTime, E> {
        text.parse().map_err(E::custom)
    }
}

impl<'de> Deserialize<'de> for ClockTime {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(ClockVisitor)
    }
}

/// A local working period on one day, written `{"from": "08:00", "to": "12:00"}`; `to` may be
/// `24:00`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingPeriod {
    /// Start of the period.
    pub from: ClockTime,
    /// End of the period, after its start.
    pub to: ClockTime,
}

impl WorkingPeriod {
    /// A period between two local times.
    #[must_use]
    pub fn new(from: ClockTime, to: ClockTime) -> Self {
        Self { from, to }
    }

    /// Start of the period.
    #[must_use]
    pub fn start(self) -> ClockTime {
        self.from
    }

    /// End of the period.
    #[must_use]
    pub fn end(self) -> ClockTime {
        self.to
    }
}
