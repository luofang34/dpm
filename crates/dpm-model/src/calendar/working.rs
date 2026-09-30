//! Working-time arithmetic on a calendar compiled into instants relative to an origin.
//!
//! Compiling reads the weekly periods and exceptions of every local date in a window, converts
//! each period to UTC in the calendar's time zone, and keeps the merged spans with the working
//! time before each. Spans, working totals and every comparison are integer milliseconds, so a
//! span boundary compares equal to itself however the origin falls, and results do not depend on
//! the window compiled. The interface speaks hours after the origin, the unit of every schedule
//! projection. A query outside the compiled window reports [`BeyondCalendar`] rather than
//! guessing, and the caller compiles a wider window up to [`MAX_WINDOW_HOURS`].

use super::{CalendarDefinition, CalendarRules, WorkingPeriod, standard_definition};
use chrono::{DateTime, Datelike, Days, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use index::Ranked;

mod arithmetic;
mod index;

/// A time or amount of working time outside the compiled window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("time lies outside the compiled calendar window")]
pub struct BeyondCalendar;

/// Longest window a calendar is compiled over: a century, of which schedule projections reach at
/// least three quarters ahead of their clock reading. Work that needs more calendar than this is
/// out of range rather than an unbounded allocation; validation refuses calendars closed for
/// longer than five years, so only extreme durations or lags reach it.
pub const MAX_WINDOW_HOURS: f64 = 24.0 * 366.0 * 100.0;

const MILLIS_PER_HOUR: f64 = 3_600_000.0;

/// Working spans of one calendar within a window, or every hour for the `always` calendar.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkingTime {
    /// First and last covered millisecond after the origin.
    window: (i64, i64),
    /// Ordered, disjoint, non-touching working spans in milliseconds after the origin.
    spans: Vec<(i64, i64)>,
    /// Working milliseconds before each span, within the window.
    before: Vec<i64>,
    /// Span starts, ranked.
    starts: Ranked,
    /// Span ends, ranked.
    ends: Ranked,
    /// Working milliseconds up to the end of each span, ranked.
    through: Ranked,
    always: bool,
}

/// Milliseconds for hours after the origin; `None` when not representable.
fn millis(hours: f64) -> Option<i64> {
    let value = (hours * MILLIS_PER_HOUR).round();
    // chrono's range is far below 2^53 ms, so this bound also keeps the cast exact.
    (value.is_finite() && value.abs() <= 9.0e15).then_some(value as i64)
}

fn hours(millis: i64) -> f64 {
    millis as f64 / MILLIS_PER_HOUR
}

/// The UTC instant of a local wall-clock time; a time skipped by a daylight-saving change moves
/// forward to the first valid local minute, and a repeated time takes its earlier occurrence.
fn local_instant(zone: Tz, local: NaiveDateTime) -> Option<DateTime<Utc>> {
    let mut candidate = local;
    // A skipped local hour is never longer than a few hours in the tz database.
    for _ in 0..=(6 * 4) {
        if let Some(at) = zone.from_local_datetime(&candidate).earliest() {
            return Some(at.with_timezone(&Utc));
        }
        candidate = candidate.checked_add_signed(chrono::TimeDelta::minutes(15))?;
    }
    None
}

fn periods_of(definition: &CalendarDefinition, date: NaiveDate) -> &[WorkingPeriod] {
    match definition.exceptions.iter().find(|e| e.covers(date)) {
        Some(exception) => &exception.hours,
        None => definition.week.day(date.weekday()),
    }
}

/// Milliseconds from the origin to a local time of day on a date.
fn local_offset(zone: Tz, origin: DateTime<Utc>, date: NaiveDate, minutes: u16) -> Option<i64> {
    let local = date
        .and_hms_opt(0, 0, 0)?
        .checked_add_signed(chrono::TimeDelta::minutes(i64::from(minutes)))?;
    Some((local_instant(zone, local)? - origin).num_milliseconds())
}

impl WorkingTime {
    /// A calendar in which every hour is working time.
    #[must_use]
    pub fn always() -> Self {
        Self {
            window: (i64::MIN, i64::MAX),
            spans: Vec::new(),
            before: Vec::new(),
            starts: Ranked::new(Vec::new(), 0, 0),
            ends: Ranked::new(Vec::new(), 0, 0),
            through: Ranked::new(Vec::new(), 0, 0),
            always: true,
        }
    }

    /// Whether every hour is working time.
    #[must_use]
    pub fn is_always(&self) -> bool {
        self.always
    }

    /// Compile the local dates covering hours `from..=until` after `origin`; `None` when the
    /// window is longer than [`MAX_WINDOW_HOURS`] or outside chrono's range.
    #[must_use]
    pub fn compile(
        rules: CalendarRules<'_>,
        zone: Tz,
        origin: DateTime<Utc>,
        from: f64,
        until: f64,
    ) -> Option<Self> {
        let definition = match rules {
            CalendarRules::Always => return Some(Self::always()),
            CalendarRules::Standard => &standard_definition(),
            CalendarRules::Weekly(definition) => definition,
        };
        let span = until - from;
        if span.is_nan() || span > MAX_WINDOW_HOURS {
            return None;
        }
        let date_at = |hours: f64| {
            crate::events::hours_after(origin, hours).map(|at| at.with_timezone(&zone).date_naive())
        };
        let first = date_at(from)?.checked_sub_days(Days::new(1))?;
        let last = date_at(until)?.checked_add_days(Days::new(1))?;
        let mut spans = Vec::new();
        let mut date = first;
        while date <= last {
            for period in periods_of(definition, date) {
                let start = local_offset(zone, origin, date, period.start().minutes())?;
                let end = local_offset(zone, origin, date, period.end().minutes())?;
                if end > start {
                    spans.push((start, end));
                }
            }
            date = date.succ_opt()?;
        }
        let window = (
            local_offset(zone, origin, first, 0)?,
            local_offset(zone, origin, last.succ_opt()?, 0)?,
        );
        Some(Self::from_spans(window, spans))
    }

    fn from_spans(window: (i64, i64), mut spans: Vec<(i64, i64)>) -> Self {
        spans.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::with_capacity(spans.len());
        for (start, end) in spans {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        let mut before = Vec::with_capacity(merged.len());
        let mut through = Vec::with_capacity(merged.len());
        let mut total: i64 = 0;
        for (start, end) in &merged {
            before.push(total);
            total = total.saturating_add(end - start);
            through.push(total);
        }
        let starts = merged.iter().map(|span| span.0).collect();
        let ends = merged.iter().map(|span| span.1).collect();
        Self {
            starts: Ranked::new(starts, window.0, window.1),
            ends: Ranked::new(ends, window.0, window.1),
            through: Ranked::new(through, 0, total),
            window,
            spans: merged,
            before,
            always: false,
        }
    }

    /// Hours after `from` at which `hours` of working time have passed, compiling wider windows
    /// until the answer is inside one. `None` when the calendar has no working time left within
    /// [`MAX_WINDOW_HOURS`].
    #[must_use]
    pub fn offset_after(
        rules: CalendarRules<'_>,
        zone: Tz,
        from: DateTime<Utc>,
        hours: f64,
    ) -> Option<f64> {
        let mut until = (hours.max(0.0) * 2.0 + 24.0 * 14.0).min(MAX_WINDOW_HOURS);
        loop {
            let calendar = Self::compile(rules, zone, from, 0.0, until)?;
            if let Ok(end) = calendar.add(0.0, hours) {
                return Some(end);
            }
            if until >= MAX_WINDOW_HOURS {
                return None;
            }
            until = (until * 2.0).min(MAX_WINDOW_HOURS);
        }
    }
}

#[cfg(test)]
mod tests;
