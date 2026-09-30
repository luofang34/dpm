//! Combine a calendar with its base into one weekly pattern with non-overlapping exceptions.
//!
//! As in Microsoft Project, a derived calendar inherits every weekday it does not list and every
//! exception of its base on dates it does not define itself. Weekdays listed nowhere in the chain
//! have Microsoft Project's defaults (Monday to Friday, 08:00-12:00 and 13:00-17:00).

use super::parse::SourceCalendar;
use crate::mspdi::report::Finding;
use chrono::{NaiveDate, Weekday};
use dpm_model::{CalendarDefinition, CalendarException, WeekPattern, WorkingPeriod};
use std::collections::BTreeMap;

/// Effective rules of one calendar and what combining it lost.
#[derive(Debug, Clone)]
pub(crate) struct Combined {
    pub(crate) definition: CalendarDefinition,
    pub(crate) approximated: Vec<Finding>,
    pub(crate) rejected: Vec<Finding>,
}

/// `DayType` codes in MSPDI order: 1 is Sunday.
const DAY_TYPES: [(u8, Weekday); 7] = [
    (1, Weekday::Sun),
    (2, Weekday::Mon),
    (3, Weekday::Tue),
    (4, Weekday::Wed),
    (5, Weekday::Thu),
    (6, Weekday::Fri),
    (7, Weekday::Sat),
];

/// The effective definition of calendar `uid`, or why it cannot be used.
pub(crate) fn combine(all: &BTreeMap<i64, &SourceCalendar>, uid: i64) -> Result<Combined, String> {
    combine_at(all, uid, 0)
}

fn combine_at(
    all: &BTreeMap<i64, &SourceCalendar>,
    uid: i64,
    depth: usize,
) -> Result<Combined, String> {
    if depth > all.len() {
        return Err("base calendars form a cycle".into());
    }
    let calendar = all
        .get(&uid)
        .ok_or_else(|| format!("calendar UID {uid} is not in the document"))?;
    if let Some(reason) = &calendar.unusable {
        return Err(reason.clone());
    }
    let base = match calendar.base_uid.filter(|_| !calendar.is_base) {
        Some(base) => Some(
            combine_at(all, base, depth + 1)
                .map_err(|reason| format!("base calendar UID {base}: {reason}"))?,
        ),
        None => None,
    };
    let inherited = base.as_ref().map_or_else(
        || dpm_model::standard_definition().week,
        |b| b.definition.week.clone(),
    );
    let mut week = WeekPattern::default();
    for (code, day) in DAY_TYPES {
        *slot(&mut week, day) = calendar
            .days
            .get(&code)
            .cloned()
            .unwrap_or_else(|| inherited.day(day).to_vec());
    }
    if week.days().all(|(_, periods)| periods.is_empty()) {
        return Err(
            "the weekly pattern has no working time, so work on it would never finish".into(),
        );
    }
    let mut combined = Combined {
        definition: CalendarDefinition {
            week,
            exceptions: Vec::new(),
        },
        approximated: calendar.approximated.clone(),
        rejected: calendar.rejected.clone(),
    };
    let own = distinct(&calendar.exceptions, &mut combined.rejected);
    let cuts: Vec<_> = own.iter().map(|e| (e.from, e.last())).collect();
    let mut exceptions = own;
    for inherited in base.iter().flat_map(|b| b.definition.exceptions.iter()) {
        for (from, last) in subtract((inherited.from, inherited.last()), &cuts) {
            exceptions.push(CalendarException {
                from,
                to: (last != from).then_some(last),
                ..inherited.clone()
            });
        }
    }
    exceptions.sort_by_key(|e| e.from);
    combined.definition.exceptions = exceptions;
    Ok(combined)
}

fn slot(week: &mut WeekPattern, day: Weekday) -> &mut Vec<WorkingPeriod> {
    match day {
        Weekday::Mon => &mut week.mon,
        Weekday::Tue => &mut week.tue,
        Weekday::Wed => &mut week.wed,
        Weekday::Thu => &mut week.thu,
        Weekday::Fri => &mut week.fri,
        Weekday::Sat => &mut week.sat,
        Weekday::Sun => &mut week.sun,
    }
}

/// Exceptions in date order; one overlapping an earlier exception is dropped and reported, since
/// the model gives each date one exception.
fn distinct(
    exceptions: &[CalendarException],
    rejected: &mut Vec<Finding>,
) -> Vec<CalendarException> {
    let mut sorted = exceptions.to_vec();
    sorted.sort_by_key(|e| e.from);
    let mut kept: Vec<CalendarException> = Vec::new();
    for exception in sorted {
        match kept.last() {
            Some(previous) if exception.from <= previous.last() => rejected.push(Finding::new(
                "exception",
                format!(
                    "exception {:?} from {} overlaps exception {:?} from {}; not imported",
                    exception.name.as_deref().unwrap_or_default(),
                    exception.from,
                    previous.name.as_deref().unwrap_or_default(),
                    previous.from
                ),
            )),
            _ => kept.push(exception),
        }
    }
    kept
}

/// Parts of an inclusive date range not covered by sorted, disjoint `cuts`.
pub(super) fn subtract(
    (from, last): (NaiveDate, NaiveDate),
    cuts: &[(NaiveDate, NaiveDate)],
) -> Vec<(NaiveDate, NaiveDate)> {
    let mut pieces = Vec::new();
    let mut start = Some(from);
    for (cut_from, cut_last) in cuts {
        let Some(begin) = start else {
            break;
        };
        if *cut_last < begin || *cut_from > last {
            continue;
        }
        if let Some(before) = cut_from.pred_opt().filter(|_| *cut_from > begin) {
            pieces.push((begin, before));
        }
        start = cut_last.succ_opt().filter(|next| *next <= last);
    }
    if let Some(begin) = start {
        pieces.push((begin, last));
    }
    pieces
}
