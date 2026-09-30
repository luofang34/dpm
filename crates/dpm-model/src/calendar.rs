//! Working calendars: when each kind of actor works, in the workspace's time zone.
//!
//! Durations and positive lags are authored as hours. Without a [`Calendars`] block every hour is
//! elapsed time. With one, an estimate counts working hours on the calendar its task resolves to,
//! reviews wait for the verifier kind's calendar, and a dependency may count its lag in working
//! time. Projections compile calendars into [`WorkingTime`]; no derived date is ever stored.

use crate::{ActorId, ActorKind, Dependency, LagBasis, Plan, WorkItem};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod clock;
mod working;
pub use clock::{ClockTime, WorkingPeriod};
pub use working::{BeyondCalendar, WorkingTime};
pub(crate) mod validate;

/// Built-in calendar counting every hour, the elapsed-time behaviour of a plan without calendars.
pub const ALWAYS: &str = "always";
/// Built-in calendar matching Microsoft Project's Standard: Monday to Friday, 08:00-12:00 and
/// 13:00-17:00. A plan may redefine it.
pub const STANDARD: &str = "standard";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Workspace calendars; present only when some work follows working time.
#[serde(deny_unknown_fields)]
pub struct Calendars {
    /// IANA time zone in which weekly hours and exception dates are read.
    pub time_zone: String,
    /// Named calendars; `standard` may be redefined, `always` may not.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub definitions: BTreeMap<String, CalendarDefinition>,
    /// Calendar of each actor kind unless a task or actor entry says otherwise.
    #[serde(default, skip_serializing_if = "KindCalendars::is_default")]
    pub kinds: KindCalendars,
    /// Availability of named actors, keyed `kind:name`; availability only, never capacity.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub actors: BTreeMap<String, String>,
    /// Actor kind expected to do work that neither names an executor nor has an owner.
    #[serde(default = "human", skip_serializing_if = "is_human")]
    pub default_executor: ActorKind,
    /// Actor kind that reviews submitted work; verification waits for its calendar.
    #[serde(default = "human", skip_serializing_if = "is_human")]
    pub verifier: ActorKind,
}

fn human() -> ActorKind {
    ActorKind::Human
}

fn is_human(kind: &ActorKind) -> bool {
    *kind == ActorKind::Human
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Default calendar name per actor kind.
#[serde(deny_unknown_fields)]
pub struct KindCalendars {
    /// People; Microsoft Project's Standard calendar unless set.
    #[serde(default = "standard_name")]
    pub human: String,
    /// Software agents; every hour unless set.
    #[serde(default = "always_name")]
    pub agent: String,
    /// External services and CI; every hour unless set.
    #[serde(default = "always_name")]
    pub service: String,
}

fn standard_name() -> String {
    STANDARD.into()
}

fn always_name() -> String {
    ALWAYS.into()
}

impl Default for KindCalendars {
    fn default() -> Self {
        Self {
            human: standard_name(),
            agent: always_name(),
            service: always_name(),
        }
    }
}

impl KindCalendars {
    /// Whether every kind uses its default calendar, which serialization omits.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Calendar name of one actor kind.
    #[must_use]
    pub fn of(&self, kind: ActorKind) -> &str {
        match kind {
            ActorKind::Human => &self.human,
            ActorKind::Agent => &self.agent,
            ActorKind::Service => &self.service,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
/// Weekly working periods and dated exceptions of one named calendar.
#[serde(deny_unknown_fields)]
pub struct CalendarDefinition {
    /// Working periods of each weekday; a day without periods is not worked.
    pub week: WeekPattern,
    /// Dates whose periods replace the weekly pattern, such as holidays or extra shifts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exceptions: Vec<CalendarException>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
/// Local working periods per weekday, ordered and non-overlapping.
#[serde(deny_unknown_fields)]
pub struct WeekPattern {
    /// Monday.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mon: Vec<WorkingPeriod>,
    /// Tuesday.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tue: Vec<WorkingPeriod>,
    /// Wednesday.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wed: Vec<WorkingPeriod>,
    /// Thursday.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thu: Vec<WorkingPeriod>,
    /// Friday.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fri: Vec<WorkingPeriod>,
    /// Saturday.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sat: Vec<WorkingPeriod>,
    /// Sunday.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sun: Vec<WorkingPeriod>,
}

impl WeekPattern {
    /// Working periods of one weekday.
    #[must_use]
    pub fn day(&self, day: chrono::Weekday) -> &[WorkingPeriod] {
        use chrono::Weekday::{Fri, Mon, Sat, Sun, Thu, Tue, Wed};
        match day {
            Mon => &self.mon,
            Tue => &self.tue,
            Wed => &self.wed,
            Thu => &self.thu,
            Fri => &self.fri,
            Sat => &self.sat,
            Sun => &self.sun,
        }
    }

    /// Every weekday with its periods, Monday first.
    pub fn days(&self) -> impl Iterator<Item = (chrono::Weekday, &[WorkingPeriod])> {
        use chrono::Weekday::{Fri, Mon, Sat, Sun, Thu, Tue, Wed};
        [Mon, Tue, Wed, Thu, Fri, Sat, Sun]
            .into_iter()
            .map(|day| (day, self.day(day)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Local dates, inclusive, whose working periods replace the weekly pattern.
#[serde(deny_unknown_fields)]
pub struct CalendarException {
    /// First date.
    pub from: chrono::NaiveDate,
    /// Last date; the exception covers only `from` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<chrono::NaiveDate>,
    /// Working periods on each covered date; empty means not worked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hours: Vec<WorkingPeriod>,
    /// Optional label such as a holiday name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl CalendarException {
    /// Last covered date.
    #[must_use]
    pub fn last(&self) -> chrono::NaiveDate {
        self.to.unwrap_or(self.from)
    }

    /// Whether the exception covers a date.
    #[must_use]
    pub fn covers(&self, date: chrono::NaiveDate) -> bool {
        self.from <= date && date <= self.last()
    }
}

/// How a calendar was chosen for work, from the most to the least specific rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarSource {
    /// The task names its own calendar.
    Task,
    /// The task's owner has an availability entry.
    Actor,
    /// The calendar of the owner's kind, else of the planned executor, else of the default.
    Kind,
}

/// Calendar and actor kind a projection uses for one work item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCalendar {
    /// Calendar name.
    pub calendar: String,
    /// Kind of the actor who owns or is expected to do the work.
    pub executor: ActorKind,
    /// Rule that chose the calendar.
    pub source: CalendarSource,
}

/// A calendar's weekly and exception rules, or a built-in.
#[derive(Debug, Clone, Copy)]
pub enum CalendarRules<'a> {
    /// Every hour is working time.
    Always,
    /// The built-in Standard calendar.
    Standard,
    /// Weekly periods and exceptions of a defined calendar.
    Weekly(&'a CalendarDefinition),
}

/// Microsoft Project's Standard calendar: Monday to Friday, 08:00-12:00 and 13:00-17:00.
#[must_use]
pub fn standard_definition() -> CalendarDefinition {
    let day = vec![
        WorkingPeriod::new(ClockTime::hours(8), ClockTime::hours(12)),
        WorkingPeriod::new(ClockTime::hours(13), ClockTime::hours(17)),
    ];
    CalendarDefinition {
        week: WeekPattern {
            mon: day.clone(),
            tue: day.clone(),
            wed: day.clone(),
            thu: day.clone(),
            fri: day,
            sat: Vec::new(),
            sun: Vec::new(),
        },
        exceptions: Vec::new(),
    }
}

impl Calendars {
    /// Calendars with Microsoft Project defaults in one time zone.
    #[must_use]
    pub fn in_zone(time_zone: impl Into<String>) -> Self {
        Self {
            time_zone: time_zone.into(),
            definitions: BTreeMap::new(),
            kinds: KindCalendars::default(),
            actors: BTreeMap::new(),
            default_executor: ActorKind::Human,
            verifier: ActorKind::Human,
        }
    }

    /// The rules of a named calendar, or `None` when the name is unknown.
    #[must_use]
    pub fn rules(&self, name: &str) -> Option<CalendarRules<'_>> {
        if name == ALWAYS {
            return Some(CalendarRules::Always);
        }
        match self.definitions.get(name) {
            Some(definition) => Some(CalendarRules::Weekly(definition)),
            None if name == STANDARD => Some(CalendarRules::Standard),
            None => None,
        }
    }

    /// Parsed time zone; validation guarantees it parses for a validated plan.
    pub fn zone(&self) -> Result<chrono_tz::Tz, BeyondCalendar> {
        self.time_zone.parse().map_err(|_| BeyondCalendar)
    }

    /// Calendar and executor kind of a work item: its own calendar, else its owner's availability,
    /// else the calendar of its owner's kind, planned executor or the default executor.
    #[must_use]
    pub fn resolve(&self, work: &WorkItem) -> ResolvedCalendar {
        let owner = work.execution.owner.as_ref();
        let executor = owner
            .map(|o| o.kind)
            .or(work.schedule.executor)
            .unwrap_or(self.default_executor);
        let actor = owner.and_then(|o| self.actors.get(&o.to_string()));
        let (calendar, source) = match (&work.schedule.calendar, actor) {
            (Some(own), _) => (own.clone(), CalendarSource::Task),
            (None, Some(available)) => (available.clone(), CalendarSource::Actor),
            (None, None) => (self.kinds.of(executor).to_owned(), CalendarSource::Kind),
        };
        ResolvedCalendar {
            calendar,
            executor,
            source,
        }
    }

    /// Name of the calendar reviews wait for.
    #[must_use]
    pub fn verifier_calendar(&self) -> &str {
        self.kinds.of(self.verifier)
    }

    /// Time at which `hours` of working time on a calendar have passed after `from`.
    ///
    /// `None` when the calendar is unknown or the result lies outside chrono's range.
    #[must_use]
    pub fn after_working(
        &self,
        calendar: &str,
        from: chrono::DateTime<chrono::Utc>,
        hours: f64,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        let rules = self.rules(calendar)?;
        let zone = self.zone().ok()?;
        let offset = WorkingTime::offset_after(rules, zone, from, hours)?;
        crate::events::hours_after(from, offset)
    }
}

impl Plan {
    /// Calendar and executor a projection uses for work, or `None` without a calendars block.
    #[must_use]
    pub fn work_calendar(&self, work: &WorkItem) -> Option<ResolvedCalendar> {
        self.calendars.as_ref().map(|c| c.resolve(work))
    }

    /// When a dependency's positive lag after `event_at` has passed: elapsed hours, or working
    /// hours of the successor's calendar for a working-time lag. `None` when out of range.
    #[must_use]
    pub fn lag_end(
        &self,
        edge: &Dependency,
        event_at: chrono::DateTime<chrono::Utc>,
        hours: f64,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        let calendars = self.calendars.as_ref();
        let successor = self.work_items.get(&edge.successor);
        match (edge.lag_basis, calendars, successor) {
            (LagBasis::Working, Some(calendars), Some(work)) => {
                calendars.after_working(&calendars.resolve(work).calendar, event_at, hours)
            }
            _ => crate::events::hours_after(event_at, hours),
        }
    }
}

/// Parse a `kind:name` availability key.
pub(crate) fn actor_key(key: &str) -> Option<ActorId> {
    key.parse().ok()
}

#[cfg(test)]
mod tests;
