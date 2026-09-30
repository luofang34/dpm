//! Microsoft Project calendars, imported into the workspace calendars when the caller names the
//! IANA time zone that MSPDI does not carry, and written back on export.
//!
//! Base calendars and calendars that the project or a task uses are imported; calendars only
//! resources use are not, because resources are not imported. A calendar named Standard with the
//! hours of the workspace's `standard` calendar maps to it, and one named 24 Hours that works
//! every hour maps to `always`; every other calendar becomes a definition under its own name.

mod definition;
pub(crate) mod export;
mod parse;

pub(crate) use parse::{SourceCalendar, parse_all};

use super::report::{CalendarOutcome, CalendarReport, Finding};
use super::source::SourceProject;
use crate::InterchangeError;
use dpm_model::{
    ALWAYS, CalendarDefinition, Calendars, ClockTime, Plan, STANDARD, WeekPattern, WorkingPeriod,
};
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

/// Candidate workspace calendars and the DPM name of every imported source calendar.
#[derive(Debug, Clone)]
pub(crate) struct Imported {
    pub(crate) calendars: Calendars,
    names: BTreeMap<i64, String>,
    /// Calendar of work that names no calendar of its own in the source.
    pub(crate) project: String,
}

impl Imported {
    /// DPM calendar of an imported source calendar UID.
    pub(crate) fn name(&self, uid: i64) -> Option<&str> {
        self.names.get(&uid).map(String::as_str)
    }
}

/// Imported calendars with one report per source calendar and document-level findings.
pub(crate) struct CalendarImport {
    pub(crate) imported: Imported,
    pub(crate) reports: Vec<CalendarReport>,
    pub(crate) findings: Vec<Finding>,
}

/// Map the document's calendars onto the workspace calendars of `current` in `time_zone`.
pub(crate) fn import(
    current: &Plan,
    source: &SourceProject,
    time_zone: &str,
) -> Result<CalendarImport, InterchangeError> {
    let mut calendars = workspace(current, time_zone)?;
    let standard = calendars
        .definitions
        .get(STANDARD)
        .cloned()
        .unwrap_or_else(dpm_model::standard_definition);
    let used: BTreeSet<i64> = source
        .calendar_uid
        .into_iter()
        .chain(source.tasks.iter().filter_map(|t| t.calendar_uid))
        .collect();
    let mut all = BTreeMap::new();
    let mut reports = Vec::new();
    for calendar in &source.calendars {
        match all.entry(calendar.uid) {
            Entry::Vacant(slot) => {
                slot.insert(calendar);
            }
            Entry::Occupied(_) => {
                reports.push(skipped(calendar, "an earlier calendar has the same UID"));
            }
        }
    }
    let mut names = BTreeMap::new();
    let mut taken = BTreeSet::new();
    for calendar in &source.calendars {
        if all
            .get(&calendar.uid)
            .is_none_or(|c| !std::ptr::eq(*c, calendar))
        {
            continue;
        }
        if !calendar.is_base && !used.contains(&calendar.uid) {
            reports.push(skipped(
                calendar,
                "resource calendar not imported; resources are not imported",
            ));
            continue;
        }
        let combined = match definition::combine(&all, calendar.uid) {
            Ok(combined) => combined,
            Err(reason) => {
                reports.push(skipped(calendar, &reason));
                continue;
            }
        };
        let (name, outcome) =
            if calendar.name.eq_ignore_ascii_case("Standard") && combined.definition == standard {
                (STANDARD.to_owned(), CalendarOutcome::BuiltIn)
            } else if calendar.name.eq_ignore_ascii_case("24 Hours")
                && combined.definition == always_definition()
            {
                (ALWAYS.to_owned(), CalendarOutcome::BuiltIn)
            } else {
                let name = unique_name(calendar, &mut taken);
                let outcome = store(&mut calendars, &name, combined.definition);
                (name, outcome)
            };
        names.insert(calendar.uid, name.clone());
        reports.push(CalendarReport {
            uid: calendar.uid,
            name: calendar.name.clone(),
            calendar: Some(name),
            outcome,
            approximated: combined.approximated,
            rejected: combined.rejected,
        });
    }
    let mut findings = Vec::new();
    let project = project_calendar(current, source, &names, &mut calendars, &mut findings);
    Ok(CalendarImport {
        imported: Imported {
            calendars,
            names,
            project,
        },
        reports,
        findings,
    })
}

/// Workspace calendars the import extends: the existing ones, which must read dates in the same
/// time zone, or Microsoft Project's defaults in `time_zone`.
fn workspace(current: &Plan, time_zone: &str) -> Result<Calendars, InterchangeError> {
    if Calendars::in_zone(time_zone).zone().is_err() {
        return Err(InterchangeError::InvalidTimeZone {
            time_zone: time_zone.into(),
        });
    }
    match &current.calendars {
        Some(existing) if existing.time_zone != time_zone => {
            Err(InterchangeError::TimeZoneMismatch {
                workspace: existing.time_zone.clone(),
                requested: time_zone.into(),
            })
        }
        Some(existing) => Ok(existing.clone()),
        None => Ok(Calendars::in_zone(time_zone)),
    }
}

/// The project calendar becomes the human calendar of a workspace that had no calendars; a
/// workspace with calendars keeps its kinds, and imported work names the calendar instead.
fn project_calendar(
    current: &Plan,
    source: &SourceProject,
    names: &BTreeMap<i64, String>,
    calendars: &mut Calendars,
    findings: &mut Vec<Finding>,
) -> String {
    match source.calendar_uid.map(|uid| (uid, names.get(&uid))) {
        Some((_, Some(name))) => {
            if current.calendars.is_none() {
                calendars.kinds.human = name.clone();
            }
            name.clone()
        }
        Some((uid, None)) => {
            findings.push(Finding::new(
                "calendars",
                format!(
                    "project calendar UID {uid} not imported; work without its own calendar follows {:?}",
                    calendars.kinds.human
                ),
            ));
            calendars.kinds.human.clone()
        }
        None => {
            findings.push(Finding::new(
                "calendars",
                format!(
                    "the document names no project calendar; work without its own calendar follows {:?}",
                    calendars.kinds.human
                ),
            ));
            calendars.kinds.human.clone()
        }
    }
}

fn skipped(calendar: &SourceCalendar, reason: &str) -> CalendarReport {
    let mut rejected = vec![Finding::new("calendar", reason)];
    rejected.extend(calendar.rejected.iter().cloned());
    CalendarReport {
        uid: calendar.uid,
        name: calendar.name.clone(),
        calendar: None,
        outcome: CalendarOutcome::Skipped,
        approximated: calendar.approximated.clone(),
        rejected,
    }
}

/// The source name, or `Calendar UID`, made unique among this import and never a built-in name.
fn unique_name(calendar: &SourceCalendar, taken: &mut BTreeSet<String>) -> String {
    let base = if calendar.name.is_empty() {
        format!("Calendar {}", calendar.uid)
    } else {
        calendar.name.clone()
    };
    let reserved = |name: &str| name == ALWAYS || name == STANDARD || taken.contains(name);
    let mut name = base.clone();
    let mut attempt: u32 = 1;
    while reserved(&name) {
        name = if attempt == 1 {
            format!("{base} (UID {})", calendar.uid)
        } else {
            format!("{base} (UID {} #{attempt})", calendar.uid)
        };
        attempt = attempt.wrapping_add(1);
    }
    taken.insert(name.clone());
    name
}

/// Store a definition; a same-named workspace calendar is replaced, since the document owns it.
fn store(calendars: &mut Calendars, name: &str, definition: CalendarDefinition) -> CalendarOutcome {
    match calendars
        .definitions
        .insert(name.to_owned(), definition.clone())
    {
        None => CalendarOutcome::Created,
        Some(previous) if previous == definition => CalendarOutcome::Unchanged,
        Some(_) => CalendarOutcome::Updated,
    }
}

/// Every hour of every day and no exceptions: the rules of the built-in `always`.
pub(crate) fn always_definition() -> CalendarDefinition {
    let day = vec![WorkingPeriod::new(
        ClockTime::hours(0),
        ClockTime::hours(24),
    )];
    CalendarDefinition {
        week: WeekPattern {
            mon: day.clone(),
            tue: day.clone(),
            wed: day.clone(),
            thu: day.clone(),
            fri: day.clone(),
            sat: day.clone(),
            sun: day,
        },
        exceptions: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
