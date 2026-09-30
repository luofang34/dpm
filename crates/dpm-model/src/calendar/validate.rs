//! Structural checks of calendars and of the calendar names work refers to.

use super::{ALWAYS, CalendarDefinition, WorkingPeriod, actor_key};
use crate::validation::invalid;
use crate::{Plan, ValidationError};

pub(crate) fn validate(plan: &Plan) -> Result<(), ValidationError> {
    let Some(calendars) = &plan.calendars else {
        return match plan
            .work_items
            .values()
            .find(|w| w.schedule.calendar.is_some())
        {
            Some(work) => Err(invalid(
                "work calendar",
                work.id,
                "a task calendar requires the plan's calendars block with a time zone",
            )),
            None => Ok(()),
        };
    };
    if calendars.time_zone.parse::<chrono_tz::Tz>().is_err() {
        return Err(invalid(
            "calendars",
            &calendars.time_zone,
            "time_zone must be an IANA time zone name such as Europe/Berlin",
        ));
    }
    for (name, definition) in &calendars.definitions {
        definition_valid(name, definition)?;
    }
    let known = |name: &str| calendars.rules(name).is_some();
    for (kind, name) in [
        ("human", &calendars.kinds.human),
        ("agent", &calendars.kinds.agent),
        ("service", &calendars.kinds.service),
    ] {
        if !known(name) {
            return Err(invalid(
                "calendar kind",
                kind,
                format!("unknown calendar {name:?}"),
            ));
        }
    }
    for (actor, name) in &calendars.actors {
        if actor_key(actor).is_none() {
            return Err(invalid(
                "calendar actor",
                actor,
                "actor keys are human:NAME, agent:NAME or service:NAME",
            ));
        }
        if !known(name) {
            return Err(invalid(
                "calendar actor",
                actor,
                format!("unknown calendar {name:?}"),
            ));
        }
    }
    for work in plan.work_items.values() {
        if let Some(name) = work.schedule.calendar.as_deref().filter(|n| !known(n)) {
            return Err(invalid(
                "work calendar",
                work.id,
                format!("unknown calendar {name:?}"),
            ));
        }
    }
    Ok(())
}

fn definition_valid(name: &str, definition: &CalendarDefinition) -> Result<(), ValidationError> {
    if name.trim().is_empty() || name == ALWAYS {
        return Err(invalid(
            "calendar",
            name,
            "a calendar needs a non-empty name other than the built-in \"always\"",
        ));
    }
    for (day, periods) in definition.week.days() {
        periods_valid(periods)
            .map_err(|reason| invalid("calendar", format!("{name} {day}"), reason))?;
    }
    if definition
        .week
        .days()
        .all(|(_, periods)| periods.is_empty())
    {
        return Err(invalid(
            "calendar",
            name,
            "the weekly pattern has no working time, so work on it would never finish",
        ));
    }
    let mut ranges: Vec<_> = definition
        .exceptions
        .iter()
        .map(|e| (e.from, e.last()))
        .collect();
    for exception in &definition.exceptions {
        let label = format!("{name} {}", exception.from);
        if exception.last() < exception.from {
            return Err(invalid(
                "calendar exception",
                label,
                "to must not be before from",
            ));
        }
        periods_valid(&exception.hours).map_err(|r| invalid("calendar exception", label, r))?;
    }
    ranges.sort();
    if let Some(pair) = ranges
        .windows(2)
        .find(|w| matches!(w, [a, b] if b.0 <= a.1))
    {
        let date = pair.first().map(|range| range.0.to_string());
        return Err(invalid(
            "calendar exception",
            format!("{name} {}", date.unwrap_or_default()),
            "exceptions of one calendar must not overlap",
        ));
    }
    Ok(())
}

fn periods_valid(periods: &[WorkingPeriod]) -> Result<(), String> {
    if let Some(period) = periods.iter().find(|p| p.start() >= p.end()) {
        return Err(format!(
            "working period {}-{} must end after it starts",
            period.start(),
            period.end()
        ));
    }
    if periods
        .windows(2)
        .any(|w| matches!(w, [a, b] if b.start() < a.end()))
    {
        return Err("working periods must be in order and must not overlap".into());
    }
    Ok(())
}
