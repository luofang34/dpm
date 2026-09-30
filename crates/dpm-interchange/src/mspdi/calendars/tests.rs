use crate::mspdi::report::CalendarOutcome;
use crate::mspdi::tests::{PROJECT_GUID, link, task, work, workspace};
use crate::{ImportOptions, ImportResult, InterchangeError, import_mspdi};
use chrono::NaiveDate;
use dpm_model::{
    ALWAYS, CalendarDefinition, CalendarException, ClockTime, LagBasis, Plan, STANDARD,
    WorkingPeriod,
};

const ZONE: &str = "Europe/Berlin";

fn period(from: u16, to: u16) -> WorkingPeriod {
    WorkingPeriod::new(ClockTime::hours(from), ClockTime::hours(to))
}

fn date(month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, month, day).expect("date")
}

fn times(spans: &[(&str, &str)]) -> String {
    let times: String = spans
        .iter()
        .map(|(from, to)| {
            format!("<WorkingTime><FromTime>{from}</FromTime><ToTime>{to}</ToTime></WorkingTime>")
        })
        .collect();
    format!("<DayWorking>1</DayWorking><WorkingTimes>{times}</WorkingTimes>")
}

fn day(code: u8, body: &str) -> String {
    format!("<WeekDay><DayType>{code}</DayType>{body}</WeekDay>")
}

const OFF: &str = "<DayWorking>0</DayWorking>";

fn standard_week() -> String {
    let office = times(&[("08:00:00", "12:00:00"), ("13:00:00", "17:00:00")]);
    let mut days = day(1, OFF);
    for code in 2..=6 {
        days.push_str(&day(code, &office));
    }
    days + &day(7, OFF)
}

fn calendar(uid: i64, name: &str, base: Option<i64>, days: &str, extra: &str) -> String {
    format!(
        "<Calendar><UID>{uid}</UID><Name>{name}</Name><IsBaseCalendar>{}</IsBaseCalendar><BaseCalendarUID>{}</BaseCalendarUID><WeekDays>{days}</WeekDays>{extra}</Calendar>",
        u8::from(base.is_none()),
        base.unwrap_or(-1)
    )
}

fn exception(name: &str, from: &str, to: &str, occurrences: u32, body: &str) -> String {
    recurrence(name, from, to, occurrences, 1, body)
}

/// An exception with a recurrence `kind`: 1 daily, 6 weekly, as Microsoft Project writes them.
fn recurrence(name: &str, from: &str, to: &str, occurrences: u32, kind: u8, body: &str) -> String {
    format!(
        "<Exception><EnteredByOccurrences>0</EnteredByOccurrences><TimePeriod><FromDate>{from}</FromDate><ToDate>{to}</ToDate></TimePeriod><Occurrences>{occurrences}</Occurrences><Name>{name}</Name><Type>{kind}</Type><Period>1</Period>{body}</Exception>"
    )
}

fn document(calendars: &[String], tasks: &[String]) -> String {
    format!(
        "<?xml version=\"1.0\"?><Project xmlns=\"http://schemas.microsoft.com/project\"><Name>Source</Name><GUID>{PROJECT_GUID}</GUID><CalendarUID>1</CalendarUID><Calendars>{}</Calendars><Tasks>{}</Tasks></Project>",
        calendars.concat(),
        tasks.concat()
    )
}

fn options(time_zone: Option<&str>) -> ImportOptions {
    ImportOptions {
        project_key: "REL".into(),
        key_prefix: Some("MSP".into()),
        match_existing_by: None,
        keep_existing_priority: false,
        time_zone: time_zone.map(str::to_owned),
    }
}

fn import(plan: &Plan, xml: &str) -> ImportResult {
    let result = import_mspdi(plan, xml, &options(Some(ZONE))).expect("import");
    result.candidate.validate().expect("valid candidate");
    result
}

fn definition<'a>(result: &'a ImportResult, name: &str) -> &'a CalendarDefinition {
    let calendars = result.candidate.calendars.as_ref().expect("calendars");
    calendars.definitions.get(name).expect("definition")
}

#[test]
fn standard_base_calendar_maps_to_the_built_in_and_becomes_the_human_calendar() {
    let xml = document(&[calendar(1, "Standard", None, &standard_week(), "")], &[]);
    let result = import(&workspace(), &xml);
    let calendars = result.candidate.calendars.as_ref().expect("calendars");
    assert_eq!(calendars.time_zone, ZONE);
    assert!(calendars.definitions.is_empty());
    assert_eq!(calendars.kinds.human, STANDARD);
    assert_eq!(result.report.calendars[0].outcome, CalendarOutcome::BuiltIn);
    assert!(
        result
            .report
            .rejected
            .iter()
            .all(|f| f.field != "calendars")
    );
    assert!(result.report.source.scope.contains("not calendar dates"));
}

#[test]
fn base_calendar_reads_days_24_00_ends_and_both_exception_forms() {
    let night = times(&[("00:00:00", "06:00:00"), ("22:00:00", "00:00:00")]);
    let mut days = String::new();
    for code in 1..=7 {
        days.push_str(&day(code, &night));
    }
    let legacy = day(
        0,
        "<TimePeriod><FromDate>2026-12-24T00:00:00</FromDate><ToDate>2026-12-26T23:59:00</ToDate></TimePeriod><DayWorking>0</DayWorking>",
    );
    let xml = document(
        &[
            calendar(1, "Night", None, &(days.clone() + &legacy), ""),
            calendar(
                2,
                "Modern",
                None,
                &(days + &legacy),
                &format!(
                    "<Exceptions>{}</Exceptions>",
                    exception(
                        "Inventory",
                        "2026-07-01T00:00:00",
                        "2026-07-02T00:00:00",
                        1,
                        &times(&[("09:00:00", "11:00:00")])
                    )
                ),
            ),
        ],
        &[],
    );
    let result = import(&workspace(), &xml);
    let night = definition(&result, "Night");
    assert_eq!(night.week.mon, [period(0, 6), period(22, 24)]);
    assert_eq!(
        night.exceptions,
        [CalendarException {
            from: date(12, 24),
            to: Some(date(12, 26)),
            hours: Vec::new(),
            name: None,
        }]
    );
    // The <Exceptions> form is authoritative; the legacy copy is not imported twice.
    let modern = definition(&result, "Modern");
    assert_eq!(
        modern.exceptions,
        [CalendarException {
            from: date(7, 1),
            to: None,
            hours: vec![period(9, 11)],
            name: Some("Inventory".into()),
        }]
    );
}

#[test]
fn derived_calendar_inherits_unlisted_days_and_base_exceptions_it_does_not_redefine() {
    let holidays = format!(
        "<Exceptions>{}</Exceptions>",
        exception(
            "Break",
            "2026-12-24T00:00:00",
            "2026-12-31T23:59:00",
            1,
            OFF
        )
    );
    let short_friday = day(6, &times(&[("08:00:00", "12:00:00")]));
    let own = format!(
        "<Exceptions>{}</Exceptions>",
        exception(
            "Shift",
            "2026-12-28T00:00:00",
            "2026-12-28T23:59:00",
            1,
            &times(&[("10:00:00", "14:00:00")])
        )
    );
    let xml = document(
        &[
            calendar(1, "Office", None, &standard_week(), &holidays),
            calendar(2, "Part time", Some(1), &short_friday, &own),
        ],
        &[task(
            1,
            1,
            "Build",
            "<Duration>PT8H0M0S</Duration><CalendarUID>2</CalendarUID>",
        )],
    );
    let result = import(&workspace(), &xml);
    let derived = definition(&result, "Part time");
    assert_eq!(derived.week.fri, [period(8, 12)]);
    assert_eq!(derived.week.mon, [period(8, 12), period(13, 17)]);
    let ranges: Vec<_> = derived
        .exceptions
        .iter()
        .map(|e| (e.from, e.last(), e.name.as_deref()))
        .collect();
    assert_eq!(
        ranges,
        [
            (date(12, 24), date(12, 27), Some("Break")),
            (date(12, 28), date(12, 28), Some("Shift")),
            (date(12, 29), date(12, 31), Some("Break")),
        ]
    );
    let build = work(&result.candidate, "MSP-1");
    assert_eq!(build.schedule.calendar.as_deref(), Some("Part time"));
    assert!(
        result.report.items[0]
            .preserved
            .contains(&"calendar".to_string())
    );
}

#[test]
fn resource_calendars_and_unusable_calendars_are_skipped_with_a_reason() {
    let xml = document(
        &[
            calendar(1, "Standard", None, &standard_week(), ""),
            calendar(2, "Engineer", Some(1), "", ""),
            calendar(3, "Never", None, &day(2, OFF), ""),
            calendar(4, "always", None, &standard_week(), ""),
        ],
        &[task(
            1,
            1,
            "Build",
            "<Duration>PT8H0M0S</Duration><CalendarUID>3</CalendarUID>",
        )],
    );
    let result = import(&workspace(), &xml);
    let outcomes: Vec<_> = result
        .report
        .calendars
        .iter()
        .map(|c| (c.outcome, c.calendar.as_deref()))
        .collect();
    assert_eq!(
        outcomes,
        [
            (CalendarOutcome::BuiltIn, Some(STANDARD)),
            (CalendarOutcome::Skipped, None),
            (CalendarOutcome::Created, Some("Never")),
            (CalendarOutcome::Created, Some("always (UID 4)")),
        ]
    );
    assert!(
        result.report.calendars[1].rejected[0]
            .detail
            .contains("resource calendar")
    );
    // Only Monday is listed as off; the other days keep Microsoft Project's defaults.
    assert_eq!(
        definition(&result, "Never").week.tue,
        [period(8, 12), period(13, 17)]
    );
    let all_off: String = (1..=7).map(|code| day(code, OFF)).collect();
    let empty = document(&[calendar(1, "Closed", None, &all_off, "")], &[]);
    let result = import(&workspace(), &empty);
    assert_eq!(result.report.calendars[0].outcome, CalendarOutcome::Skipped);
    assert!(
        result.report.rejected[0]
            .detail
            .contains("project calendar UID 1 not imported")
    );
    assert_eq!(
        result.candidate.calendars.expect("calendars").kinds.human,
        STANDARD
    );
}

#[test]
fn elapsed_durations_follow_always_and_working_ones_their_calendar() {
    let xml = document(
        &[
            calendar(1, "Standard", None, &standard_week(), ""),
            calendar(
                2,
                "Late",
                None,
                &day(2, &times(&[("14:00:00", "22:00:00")])),
                "",
            ),
        ],
        &[
            task(
                1,
                1,
                "Cure",
                "<Duration>PT48H0M0S</Duration><DurationFormat>6</DurationFormat><CalendarUID>2</CalendarUID>",
            ),
            task(
                2,
                1,
                "Paint",
                "<Duration>PT16H0M0S</Duration><DurationFormat>7</DurationFormat>",
            ),
            task(
                3,
                1,
                "Late",
                "<Duration>PT8H0M0S</Duration><CalendarUID>2</CalendarUID>",
            ),
            task(
                4,
                1,
                "Lost",
                "<Duration>PT8H0M0S</Duration><CalendarUID>9</CalendarUID>",
            ),
        ],
    );
    let result = import(&workspace(), &xml);
    let calendar = |key| work(&result.candidate, key).schedule.calendar.clone();
    assert_eq!(calendar("MSP-1").as_deref(), Some(ALWAYS));
    assert_eq!(calendar("MSP-2"), None);
    assert_eq!(calendar("MSP-3").as_deref(), Some("Late"));
    assert_eq!(calendar("MSP-4"), None);
    let paint = &result.report.items[1];
    assert!(paint.rejected.is_empty());
    assert!(
        paint
            .approximated
            .iter()
            .all(|f| !f.detail.contains("calendar not applied"))
    );
    assert!(
        result.report.items[3]
            .approximated
            .iter()
            .any(|f| f.field == "calendar")
    );
}

#[test]
fn working_lags_count_working_time_and_elapsed_lags_stay_elapsed() {
    let xml = document(
        &[calendar(1, "Standard", None, &standard_week(), "")],
        &[
            task(1, 1, "A", "<Duration>PT8H0M0S</Duration>"),
            task(
                2,
                1,
                "B",
                &format!("<Duration>PT8H0M0S</Duration>{}", link(1, 1, 4800, 7)),
            ),
            task(
                3,
                1,
                "C",
                &format!("<Duration>PT8H0M0S</Duration>{}", link(1, 1, 1200, 6)),
            ),
            task(
                4,
                1,
                "D",
                "<Duration>PT8H0M0S</Duration><PredecessorLink><PredecessorUID>1</PredecessorUID><Type>1</Type><LinkLag>600</LinkLag></PredecessorLink>",
            ),
        ],
    );
    let result = import(&workspace(), &xml);
    let bases: Vec<_> = result
        .candidate
        .dependencies
        .iter()
        .map(|d| (d.lag_hours, d.lag_basis))
        .collect();
    assert_eq!(
        bases,
        [
            (8.0, LagBasis::Working),
            (2.0, LagBasis::Elapsed),
            (1.0, LagBasis::Working)
        ]
    );
    for link in &result.report.links {
        assert_eq!(link.outcome, crate::LinkOutcome::Preserved, "{link:?}");
    }
}

#[test]
fn without_a_time_zone_calendars_are_reported_and_values_stay_elapsed() {
    let xml = document(
        &[calendar(1, "Standard", None, &standard_week(), "")],
        &[task(
            1,
            1,
            "A",
            "<Duration>PT8H0M0S</Duration><CalendarUID>1</CalendarUID>",
        )],
    );
    let result = import_mspdi(&workspace(), &xml, &options(None)).expect("import");
    assert!(result.candidate.calendars.is_none() && result.report.calendars.is_empty());
    assert!(
        result.report.rejected[0]
            .detail
            .contains("pass one (--time-zone / time_zone)")
    );
    let item = &result.report.items[0];
    assert!(item.rejected.iter().any(|f| f.field == "calendar"));
    assert!(
        item.approximated
            .iter()
            .any(|f| f.detail.contains("calendar not applied"))
    );
}

#[test]
fn time_zones_are_validated_and_must_match_the_workspace() {
    let xml = document(&[calendar(1, "Standard", None, &standard_week(), "")], &[]);
    let error = import_mspdi(&workspace(), &xml, &options(Some("Mars/Base"))).expect_err("zone");
    assert!(matches!(error, InterchangeError::InvalidTimeZone { .. }));
    assert_eq!(error.code(), "invalid_request");
    let mut plan = workspace();
    plan.calendars = Some(dpm_model::Calendars::in_zone("Asia/Tokyo"));
    let error = import_mspdi(&plan, &xml, &options(Some(ZONE))).expect_err("mismatch");
    assert!(matches!(error, InterchangeError::TimeZoneMismatch { .. }));
}

mod exceptions;
