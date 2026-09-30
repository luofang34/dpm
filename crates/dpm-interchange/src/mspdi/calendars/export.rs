//! Write the calendars exported work uses as MSPDI base calendars.
//!
//! The human kind's calendar is the project calendar (UID 1); every other calendar an exported item
//! resolves to follows in name order. The built-ins are written as Microsoft Project's own:
//! `standard` as Standard and `always` as a 24-hour, 7-day calendar named 24 Hours.

use super::always_definition;
use crate::mspdi::encoding::ELAPSED_HOURS_FORMAT;
use crate::mspdi::export::{escape, push_line};
use crate::mspdi::report::Finding;
use dpm_model::{
    ALWAYS, ActorKind, CalendarDefinition, CalendarRules, Calendars, ClockTime, Dependency,
    LagBasis, STANDARD, WorkItem, WorkKind, WorkingPeriod,
};
use std::collections::{BTreeMap, BTreeSet};

/// MSPDI duration/lag format code for working hours.
const WORKING_HOURS_FORMAT: u32 = 5;

/// Calendar numbering and time formats of one export with workspace calendars.
pub(crate) struct CalendarExport<'a> {
    calendars: &'a Calendars,
    /// MSPDI UID and written name of each used calendar, by DPM name.
    written: BTreeMap<String, (u32, String)>,
}

impl<'a> CalendarExport<'a> {
    /// Numbering for the project calendar and every calendar `work` resolves to.
    pub(crate) fn new<'w>(
        calendars: &'a Calendars,
        work: impl Iterator<Item = &'w WorkItem>,
    ) -> Self {
        let human = calendars.kinds.human.clone();
        let others: BTreeSet<String> = work
            .filter(|w| w.kind != WorkKind::WorkPackage)
            .map(|w| calendars.resolve(w).calendar)
            .filter(|name| *name != human)
            .collect();
        let mut written = BTreeMap::new();
        let mut shown = BTreeSet::new();
        for (index, name) in std::iter::once(human).chain(others).enumerate() {
            let uid = u32::try_from(index + 1).unwrap_or(u32::MAX);
            let label = match name.as_str() {
                STANDARD => "Standard".to_owned(),
                ALWAYS => "24 Hours".to_owned(),
                other => other.to_owned(),
            };
            let label = if shown.insert(label.clone()) {
                label
            } else {
                format!("{label} ({uid})")
            };
            written.insert(name, (uid, label));
        }
        Self { calendars, written }
    }

    /// UID of the project calendar: the human kind's calendar.
    pub(crate) fn project_uid(&self) -> u32 {
        self.uid(&self.calendars.kinds.human).unwrap_or(1)
    }

    fn uid(&self, name: &str) -> Option<u32> {
        self.written.get(name).map(|(uid, _)| *uid)
    }

    /// Task calendar UID when the item's calendar differs from the project calendar.
    pub(crate) fn task_uid(&self, work: &WorkItem) -> Option<u32> {
        let resolved = self.calendars.resolve(work).calendar;
        (resolved != self.calendars.kinds.human)
            .then(|| self.uid(&resolved))
            .flatten()
    }

    /// Working hours, or elapsed hours for work on `always`.
    pub(crate) fn duration_format(&self, work: &WorkItem) -> u32 {
        if self.calendars.resolve(work).calendar == ALWAYS {
            ELAPSED_HOURS_FORMAT
        } else {
            WORKING_HOURS_FORMAT
        }
    }

    /// Working hours for a working-time lag, elapsed hours otherwise.
    pub(crate) fn lag_format(edge: &Dependency) -> u32 {
        match edge.lag_basis {
            LagBasis::Working => WORKING_HOURS_FORMAT,
            LagBasis::Elapsed => ELAPSED_HOURS_FORMAT,
        }
    }

    /// The `<Calendars>` element.
    pub(crate) fn write(&self, xml: &mut String) -> Result<(), String> {
        let mut ordered: Vec<_> = self.written.iter().collect();
        ordered.sort_by_key(|(_, (uid, _))| *uid);
        push_line(xml, 1, "<Calendars>");
        for (name, (uid, label)) in ordered {
            let definition = match self.calendars.rules(name) {
                Some(CalendarRules::Weekly(definition)) => definition.clone(),
                Some(CalendarRules::Standard) | None => dpm_model::standard_definition(),
                Some(CalendarRules::Always) => always_definition(),
            };
            push_line(xml, 2, "<Calendar>");
            push_line(xml, 3, &format!("<UID>{uid}</UID>"));
            push_line(xml, 3, &format!("<Name>{}</Name>", escape(label)?));
            push_line(xml, 3, "<IsBaseCalendar>1</IsBaseCalendar>");
            push_line(xml, 3, "<BaseCalendarUID>-1</BaseCalendarUID>");
            write_definition(xml, &definition)?;
            push_line(xml, 2, "</Calendar>");
        }
        push_line(xml, 1, "</Calendars>");
        Ok(())
    }
}

fn write_definition(xml: &mut String, definition: &CalendarDefinition) -> Result<(), String> {
    push_line(xml, 3, "<WeekDays>");
    let week = &definition.week;
    let days = [
        &week.sun, &week.mon, &week.tue, &week.wed, &week.thu, &week.fri, &week.sat,
    ];
    for (code, periods) in (1..).zip(days) {
        push_line(
            xml,
            4,
            &format!(
                "<WeekDay><DayType>{code}</DayType>{}</WeekDay>",
                working(periods)
            ),
        );
    }
    push_line(xml, 3, "</WeekDays>");
    if definition.exceptions.is_empty() {
        return Ok(());
    }
    push_line(xml, 3, "<Exceptions>");
    for exception in &definition.exceptions {
        let name = match &exception.name {
            Some(name) => format!("<Name>{}</Name>", escape(name)?),
            None => String::new(),
        };
        push_line(
            xml,
            4,
            &format!(
                "<Exception><EnteredByOccurrences>0</EnteredByOccurrences><TimePeriod><FromDate>{}T00:00:00</FromDate><ToDate>{}T23:59:00</ToDate></TimePeriod><Occurrences>1</Occurrences>{name}<Type>1</Type>{}</Exception>",
                exception.from,
                exception.last(),
                working(&exception.hours)
            ),
        );
    }
    push_line(xml, 3, "</Exceptions>");
    Ok(())
}

/// `DayWorking` and `WorkingTimes` of one day; `24:00` is written as `00:00:00`.
fn working(periods: &[WorkingPeriod]) -> String {
    if periods.is_empty() {
        return "<DayWorking>0</DayWorking>".into();
    }
    let time = |clock: ClockTime| {
        let minutes = clock.minutes() % (24 * 60);
        format!("{:02}:{:02}:00", minutes / 60, minutes % 60)
    };
    let times: String = periods
        .iter()
        .map(|p| {
            format!(
                "<WorkingTime><FromTime>{}</FromTime><ToTime>{}</ToTime></WorkingTime>",
                time(p.start()),
                time(p.end())
            )
        })
        .collect();
    format!("<DayWorking>1</DayWorking><WorkingTimes>{times}</WorkingTimes>")
}

impl CalendarExport<'_> {
    /// Workspace calendar data that MSPDI does not carry.
    pub(crate) fn omissions(&self) -> Vec<Finding> {
        let calendars = self.calendars;
        let mut omitted = vec![
            Finding::new(
                "time_zone",
                format!(
                    "time zone {} is not represented in MSPDI; pass it when importing the document",
                    calendars.time_zone
                ),
            ),
            Finding::new(
                "calendar_kinds",
                format!(
                    "calendars per actor kind are not represented: the human calendar {:?} is the project calendar, and each task names the calendar it resolves to now",
                    calendars.kinds.human
                ),
            ),
        ];
        if !calendars.actors.is_empty() {
            omitted.push(Finding::new(
                "calendar_actors",
                format!(
                    "availability of {} named actor(s) is not represented",
                    calendars.actors.len()
                ),
            ));
        }
        if calendars.default_executor != ActorKind::Human || calendars.verifier != ActorKind::Human
        {
            omitted.push(Finding::new(
                "calendar_roles",
                format!(
                    "default executor {:?} and verifier {:?} are not represented",
                    calendars.default_executor, calendars.verifier
                ),
            ));
        }
        let unused = calendars
            .definitions
            .keys()
            .filter(|name| !self.written.contains_key(*name))
            .count();
        if unused > 0 {
            omitted.push(Finding::new(
                "calendars",
                format!("{unused} calendar(s) no exported work uses are not written"),
            ));
        }
        omitted
    }
}
