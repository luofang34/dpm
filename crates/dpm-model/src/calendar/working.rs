//! Working-time arithmetic on a calendar compiled into instants relative to an origin.
//!
//! Compiling reads the weekly periods and exceptions of every local date in a window, converts
//! each period to UTC in the calendar's time zone, and keeps the merged spans with the working
//! hours before each, so every query is a binary search. Times are hours after the origin, the
//! unit of every schedule projection. A query outside the compiled window reports
//! [`BeyondCalendar`] rather than guessing, and the caller compiles a wider window.

use super::{CalendarDefinition, CalendarRules, WorkingPeriod, standard_definition};
use chrono::{DateTime, Datelike, Days, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use index::Ranked;

mod index;

/// A time or amount of working time outside the compiled window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("time lies outside the compiled calendar window")]
pub struct BeyondCalendar;

/// Working spans of one calendar within a window, or every hour for the `always` calendar.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkingTime {
    /// First and last covered hour; unbounded for `always`.
    window: (f64, f64),
    /// Ordered, disjoint, non-touching working spans in hours after the origin.
    spans: Vec<(f64, f64)>,
    /// Working hours before each span, within the window.
    before: Vec<f64>,
    /// Span starts, ranked by hour.
    starts: Ranked,
    /// Span ends, ranked by hour.
    ends: Ranked,
    /// Working hours up to the end of each span, ranked by working hour.
    through: Ranked,
    always: bool,
}

const MILLIS_PER_HOUR: f64 = 3_600_000.0;

/// Hours from `origin` to `at`.
fn hours_between(origin: DateTime<Utc>, at: DateTime<Utc>) -> f64 {
    (at - origin).num_milliseconds() as f64 / MILLIS_PER_HOUR
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

impl WorkingTime {
    /// A calendar in which every hour is working time.
    #[must_use]
    pub fn always() -> Self {
        Self {
            window: (f64::NEG_INFINITY, f64::INFINITY),
            spans: Vec::new(),
            before: Vec::new(),
            starts: Ranked::new(Vec::new(), 0.0, 0.0),
            ends: Ranked::new(Vec::new(), 0.0, 0.0),
            through: Ranked::new(Vec::new(), 0.0, 0.0),
            always: true,
        }
    }

    /// Whether every hour is working time.
    #[must_use]
    pub fn is_always(&self) -> bool {
        self.always
    }

    /// Compile the local dates covering hours `from..=until` after `origin`.
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
        let first = crate::events::hours_after(origin, from)?
            .with_timezone(&zone)
            .date_naive()
            .checked_sub_days(Days::new(1))?;
        let last = crate::events::hours_after(origin, until)?
            .with_timezone(&zone)
            .date_naive()
            .checked_add_days(Days::new(1))?;
        let mut spans: Vec<(f64, f64)> = Vec::new();
        let mut date = first;
        while date <= last {
            for period in periods_of(definition, date) {
                let at = |minutes: u16| {
                    let midnight = date.and_hms_opt(0, 0, 0)?;
                    let local = midnight
                        .checked_add_signed(chrono::TimeDelta::minutes(i64::from(minutes)))?;
                    local_instant(zone, local).map(|utc| hours_between(origin, utc))
                };
                let (start, end) = (at(period.start().minutes())?, at(period.end().minutes())?);
                if end > start {
                    spans.push((start, end));
                }
            }
            date = date.succ_opt()?;
        }
        let window = (
            hours_between(origin, local_instant(zone, first.and_hms_opt(0, 0, 0)?)?),
            hours_between(
                origin,
                local_instant(zone, last.succ_opt()?.and_hms_opt(0, 0, 0)?)?,
            ),
        );
        Some(Self::from_spans(window, spans))
    }

    fn from_spans(window: (f64, f64), mut spans: Vec<(f64, f64)>) -> Self {
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64)> = Vec::with_capacity(spans.len());
        for (start, end) in spans {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        let mut before = Vec::with_capacity(merged.len());
        let mut through = Vec::with_capacity(merged.len());
        let mut total = 0.0;
        for (start, end) in &merged {
            before.push(total);
            total += end - start;
            through.push(total);
        }
        let starts = merged.iter().map(|span| span.0).collect();
        let ends = merged.iter().map(|span| span.1).collect();
        Self {
            starts: Ranked::new(starts, window.0, window.1),
            ends: Ranked::new(ends, window.0, window.1),
            through: Ranked::new(through, 0.0, total),
            window,
            spans: merged,
            before,
            always: false,
        }
    }

    /// Working hours after `from` at which `hours` of working time have passed, compiling wider
    /// windows until the answer is inside one. `None` when the calendar has no working time left
    /// within chrono's range.
    #[must_use]
    pub fn offset_after(
        rules: CalendarRules<'_>,
        zone: Tz,
        from: DateTime<Utc>,
        hours: f64,
    ) -> Option<f64> {
        let mut until = hours.max(0.0) * 2.0 + 24.0 * 14.0;
        for _ in 0..24 {
            let calendar = Self::compile(rules, zone, from, 0.0, until)?;
            if let Ok(end) = calendar.add(0.0, hours) {
                return Some(end);
            }
            until *= 2.0;
        }
        None
    }

    fn check(&self, t: f64) -> Result<(), BeyondCalendar> {
        if t.is_nan() || t < self.window.0 || t > self.window.1 {
            return Err(BeyondCalendar);
        }
        Ok(())
    }

    fn span(&self, index: usize) -> Result<((f64, f64), f64), BeyondCalendar> {
        match (self.spans.get(index), self.before.get(index)) {
            (Some(span), Some(before)) => Ok((*span, *before)),
            _ => Err(BeyondCalendar),
        }
    }

    /// Working hours between the window start and `t`.
    fn worked(&self, t: f64) -> Result<f64, BeyondCalendar> {
        self.check(t)?;
        let count = self.starts.through(t);
        let Some(index) = count.checked_sub(1) else {
            return Ok(0.0);
        };
        let ((start, end), before) = self.span(index)?;
        Ok(before + t.min(end) - start)
    }

    /// Earliest time by which `hours` of working time after `t` have passed.
    pub fn add(&self, t: f64, hours: f64) -> Result<f64, BeyondCalendar> {
        if self.always || hours <= 0.0 {
            return Ok(if self.always { t + hours } else { t });
        }
        let target = self.worked(t)? + hours;
        let index = self.through.below(target);
        let ((start, _), before) = self.span(index)?;
        let end = (start + (target - before)).max(t);
        self.check(end)?;
        Ok(end)
    }

    /// Latest time from which `hours` of working time reach `t`.
    pub fn sub(&self, t: f64, hours: f64) -> Result<f64, BeyondCalendar> {
        if self.always || hours <= 0.0 {
            return Ok(if self.always { t - hours } else { t });
        }
        let target = self.worked(t)? - hours;
        if target < 0.0 {
            return Err(BeyondCalendar);
        }
        let index = self.through.through(target);
        let ((start, _), before) = self.span(index)?;
        Ok((start + (target - before)).min(t))
    }

    /// `add` for a non-negative amount and `sub` for a negative one.
    pub fn shift(&self, t: f64, hours: f64) -> Result<f64, BeyondCalendar> {
        if hours >= 0.0 {
            self.add(t, hours)
        } else {
            self.sub(t, -hours)
        }
    }

    /// Earliest working moment at or after `t`; span ends count as working.
    pub fn align(&self, t: f64) -> Result<f64, BeyondCalendar> {
        if self.always {
            return Ok(t);
        }
        self.check(t)?;
        let index = self.ends.below(t);
        let ((start, _), _) = self.span(index)?;
        Ok(start.max(t))
    }

    /// Earliest moment at or after `t` that working time follows; a span's end moves to the next
    /// span, so work starting there is placed where it actually begins.
    pub fn next_working(&self, t: f64) -> Result<f64, BeyondCalendar> {
        if self.always {
            return Ok(t);
        }
        self.check(t)?;
        let index = self.ends.through(t);
        let ((start, _), _) = self.span(index)?;
        Ok(start.max(t))
    }

    /// Latest moment at or before `t` whose `align` is not after `t`.
    pub fn align_back(&self, t: f64) -> Result<f64, BeyondCalendar> {
        if self.always {
            return Ok(t);
        }
        self.check(t)?;
        let count = self.starts.through(t);
        let index = count.checked_sub(1).ok_or(BeyondCalendar)?;
        let ((_, end), _) = self.span(index)?;
        Ok(t.min(end))
    }

    /// Working hours between `a` and a later `b`.
    pub fn between(&self, a: f64, b: f64) -> Result<f64, BeyondCalendar> {
        if self.always {
            return Ok((b - a).max(0.0));
        }
        Ok((self.worked(b)? - self.worked(a)?).max(0.0))
    }
}

#[cfg(test)]
mod tests;
