//! Read `<Calendar>` elements as written: listed weekdays, working times and dated exceptions.
//!
//! Nothing here inherits or names anything; [`super::definition`] combines a derived calendar with
//! its base. Values the model cannot carry exactly are reported on the calendar.

use crate::mspdi::report::Finding;
use crate::mspdi::source::{children, flag, text};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use dpm_model::{CalendarException, ClockTime, WorkingPeriod};
use roxmltree::Node;
use std::collections::BTreeMap;

/// Minutes in a day; a working time ending at `00:00:00` ends at the next midnight.
const DAY_MINUTES: u16 = 24 * 60;

/// One `<Calendar>` element.
#[derive(Debug, Clone)]
pub(crate) struct SourceCalendar {
    pub(crate) uid: i64,
    pub(crate) name: String,
    pub(crate) is_base: bool,
    /// Calendar whose week and exceptions this one inherits; `None` for `-1` or absent.
    pub(crate) base_uid: Option<i64>,
    /// Working periods of each listed `DayType`, 1 (Sunday) to 7 (Saturday); unlisted days inherit.
    pub(crate) days: BTreeMap<u8, Vec<WorkingPeriod>>,
    /// Simple dated exceptions in document order, before overlaps are resolved.
    pub(crate) exceptions: Vec<CalendarException>,
    /// Values carried with a documented loss.
    pub(crate) approximated: Vec<Finding>,
    /// Values present in the document that the calendar does not carry.
    pub(crate) rejected: Vec<Finding>,
    /// Why the calendar cannot be used at all.
    pub(crate) unusable: Option<String>,
}

/// Every `<Calendar>` of the document, in document order.
pub(crate) fn parse_all(root: Node) -> Vec<SourceCalendar> {
    children(root, "Calendars")
        .flat_map(|c| children(c, "Calendar"))
        .enumerate()
        .map(|(index, node)| parse(node, index))
        .collect()
}

fn parse(node: Node, index: usize) -> SourceCalendar {
    let integer = |name: &'static str| text(node, name).and_then(|v| v.trim().parse::<i64>().ok());
    let mut calendar = SourceCalendar {
        uid: integer("UID").unwrap_or(-1),
        name: text(node, "Name").unwrap_or_default().trim().to_owned(),
        is_base: flag(node, "IsBaseCalendar"),
        base_uid: integer("BaseCalendarUID").filter(|uid| *uid >= 0),
        days: BTreeMap::new(),
        exceptions: Vec::new(),
        approximated: Vec::new(),
        rejected: Vec::new(),
        unusable: None,
    };
    if integer("UID").is_none() {
        calendar.unusable = Some(format!("calendar {} has no integer UID", index + 1));
        return calendar;
    }
    if let Err(reason) = read_content(node, &mut calendar) {
        calendar.unusable = Some(reason);
    }
    calendar
}

fn read_content(node: Node, calendar: &mut SourceCalendar) -> Result<(), String> {
    // Microsoft Project 2007 and later write each exception twice: as `<Exceptions>` and as a
    // legacy `DayType` 0 week day. The `<Exceptions>` form is authoritative when present.
    let modern = children(node, "Exceptions").next().is_some();
    for day in children(node, "WeekDays").flat_map(|d| children(d, "WeekDay")) {
        let day_type = text(day, "DayType").and_then(|v| v.trim().parse::<i64>().ok());
        match day_type {
            Some(0) if modern => {}
            Some(0) => {
                if let Some(exception) = exception(day, calendar)? {
                    calendar.exceptions.push(exception);
                }
            }
            Some(code @ 1..=7) => {
                let periods = periods(day, calendar)?;
                calendar
                    .days
                    .insert(u8::try_from(code).unwrap_or_default(), periods);
            }
            other => calendar.rejected.push(Finding::new(
                "week_day",
                format!("week day with DayType {other:?} not imported"),
            )),
        }
    }
    for node in children(node, "Exceptions").flat_map(|e| children(e, "Exception")) {
        let occurrences = text(node, "Occurrences").and_then(|v| v.trim().parse::<i64>().ok());
        if occurrences.is_some_and(|n| n > 1) {
            calendar.rejected.push(Finding::new(
                "exception",
                format!(
                    "recurring exception {:?} (recurrence type {}, {} occurrences) not imported; only single date ranges are",
                    text(node, "Name").unwrap_or_default(),
                    text(node, "Type").unwrap_or("absent"),
                    occurrences.unwrap_or_default()
                ),
            ));
            continue;
        }
        if let Some(exception) = exception(node, calendar)? {
            calendar.exceptions.push(exception);
        }
    }
    let work_weeks = children(node, "WorkWeeks")
        .flat_map(|w| children(w, "WorkWeek"))
        .count();
    if work_weeks > 0 {
        calendar.rejected.push(Finding::new(
            "work_weeks",
            format!("{work_weeks} alternate work week(s) not imported; the default week applies"),
        ));
    }
    Ok(())
}

/// A dated exception from a `TimePeriod`, or `None` when it has none (reported).
fn exception(
    node: Node,
    calendar: &mut SourceCalendar,
) -> Result<Option<CalendarException>, String> {
    let name = text(node, "Name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned);
    let period = children(node, "TimePeriod").next();
    let (Some(from), Some(to)) = (
        period.and_then(|p| text(p, "FromDate")),
        period.and_then(|p| text(p, "ToDate")),
    ) else {
        calendar.rejected.push(Finding::new(
            "exception",
            format!("exception {name:?} without a TimePeriod not imported"),
        ));
        return Ok(None);
    };
    let from = moment(from)?.date();
    let end = moment(to)?;
    // An end at midnight after the first day is exclusive: the previous day is the last one.
    let last = match end.date().pred_opt() {
        Some(previous) if end.time() == NaiveTime::MIN && end.date() > from => previous,
        _ => end.date(),
    };
    if last < from {
        return Err(format!("exception {name:?} ends before it starts"));
    }
    Ok(Some(CalendarException {
        from,
        to: (last != from).then_some(last),
        hours: periods(node, calendar)?,
        name,
    }))
}

fn moment(value: &str) -> Result<NaiveDateTime, String> {
    let value = value.trim();
    NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d").map(|d| d.and_time(NaiveTime::MIN))
        })
        .map_err(|_| format!("date {value:?} is not an MSPDI date"))
}

/// Working periods of a week day or exception, sorted with overlaps merged; a working day
/// without working times has Microsoft Project's default hours.
fn periods(node: Node, calendar: &mut SourceCalendar) -> Result<Vec<WorkingPeriod>, String> {
    if !flag(node, "DayWorking") {
        return Ok(Vec::new());
    }
    let mut spans = Vec::new();
    for time in children(node, "WorkingTimes").flat_map(|w| children(w, "WorkingTime")) {
        let (Some(from), Some(to)) = (text(time, "FromTime"), text(time, "ToTime")) else {
            continue;
        };
        let from = minutes(from, calendar)?;
        let to = match minutes(to, calendar)? {
            0 => DAY_MINUTES,
            to => to,
        };
        if from >= to {
            return Err(format!(
                "working time {} to {} does not end after it starts on the same day",
                clock(from),
                clock(to)
            ));
        }
        spans.push((from, to));
    }
    if spans.is_empty() {
        spans = vec![(8 * 60, 12 * 60), (13 * 60, 17 * 60)];
    }
    spans.sort_unstable();
    let mut merged: Vec<(u16, u16)> = Vec::new();
    for (from, to) in spans {
        match merged.last_mut() {
            Some(last) if from < last.1 => last.1 = last.1.max(to),
            _ => merged.push((from, to)),
        }
    }
    Ok(merged
        .into_iter()
        .map(|(from, to)| WorkingPeriod::new(clock(from), clock(to)))
        .collect())
}

fn clock(minutes: u16) -> ClockTime {
    ClockTime::from_minutes(minutes.min(DAY_MINUTES)).unwrap_or(ClockTime::hours(24))
}

/// Minutes after midnight of an `HH:MM:SS` time; seconds are dropped with a finding.
fn minutes(value: &str, calendar: &mut SourceCalendar) -> Result<u16, String> {
    let invalid = || format!("time {value:?} is not HH:MM:SS");
    let mut parts = value.trim().split(':');
    let mut next = || -> Result<u16, String> {
        parts
            .next()
            .unwrap_or("0")
            .parse::<u16>()
            .map_err(|_| invalid())
    };
    let (hours, minutes, seconds) = (next()?, next()?, next()?);
    if hours > 24 || minutes >= 60 || seconds >= 60 || (hours == 24 && minutes + seconds > 0) {
        return Err(invalid());
    }
    if seconds > 0 {
        calendar.approximated.push(Finding::new(
            "working_time",
            format!("time {value} rounded down to the minute"),
        ));
    }
    Ok(hours * 60 + minutes)
}
