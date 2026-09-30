use super::*;
use crate::{CalendarException, ClockTime, WorkingPeriod};
use chrono::TimeZone;

/// Monday 2 March 2026, 00:00 in New York (EST, UTC-5); DST starts on Sunday 8 March.
fn origin() -> DateTime<Utc> {
    chrono_tz::America::New_York
        .with_ymd_and_hms(2026, 3, 2, 0, 0, 0)
        .single()
        .expect("origin")
        .with_timezone(&Utc)
}

fn standard(until: f64) -> WorkingTime {
    WorkingTime::compile(
        CalendarRules::Standard,
        chrono_tz::America::New_York,
        origin(),
        0.0,
        until,
    )
    .expect("compiled")
}

#[test]
fn standard_days_follow_ms_project_hours() {
    let calendar = standard(24.0 * 21.0);
    assert_eq!(
        calendar.add(0.0, 8.0),
        Ok(17.0),
        "one day ends Monday 17:00"
    );
    assert_eq!(
        calendar.add(17.0, 1.0),
        Ok(33.0),
        "the next hour is Tuesday 09:00"
    );
    assert_eq!(
        calendar.add(0.0, 40.0),
        Ok(4.0 * 24.0 + 17.0),
        "a week ends Friday 17:00"
    );
    assert_eq!(calendar.add(10.0, 3.0), Ok(14.0), "lunch is skipped");
    assert_eq!(
        calendar.sub(17.0, 8.0),
        Ok(8.0),
        "latest start is Monday 08:00"
    );
    assert_eq!(calendar.sub(33.0, 1.0), Ok(32.0));
    assert_eq!(calendar.between(0.0, 33.0), Ok(9.0));
    assert_eq!(calendar.shift(33.0, -1.0), Ok(32.0));
}

#[test]
fn alignment_treats_span_ends_as_working() {
    let calendar = standard(24.0 * 7.0);
    assert_eq!(
        calendar.align(18.0),
        Ok(32.0),
        "evening waits for Tuesday 08:00"
    );
    assert_eq!(calendar.align(12.5), Ok(13.0));
    assert_eq!(calendar.align(17.0), Ok(17.0));
    assert_eq!(
        calendar.next_working(17.0),
        Ok(32.0),
        "work cannot begin at closing time"
    );
    assert_eq!(calendar.next_working(9.0), Ok(9.0));
    assert_eq!(calendar.align_back(18.0), Ok(17.0));
    assert_eq!(calendar.align_back(12.5), Ok(12.0));
    assert_eq!(calendar.align_back(9.0), Ok(9.0));
}

#[test]
fn daylight_saving_moves_local_hours_in_utc() {
    let calendar = standard(24.0 * 21.0);
    // Friday 17:00 EST plus one hour is Monday 09:00 EDT, one UTC hour earlier than a plain week.
    assert_eq!(calendar.add(4.0 * 24.0 + 17.0, 1.0), Ok(7.0 * 24.0 + 8.0));
}

#[test]
fn exceptions_replace_the_weekly_pattern() {
    let mut definition = crate::standard_definition();
    definition.exceptions.push(CalendarException {
        from: NaiveDate::from_ymd_opt(2026, 3, 3).expect("date"),
        to: None,
        hours: Vec::new(),
        name: Some("closed".into()),
    });
    definition.exceptions.push(CalendarException {
        from: NaiveDate::from_ymd_opt(2026, 3, 7).expect("date"),
        to: None,
        hours: vec![WorkingPeriod::new(
            ClockTime::hours(10),
            ClockTime::hours(12),
        )],
        name: None,
    });
    let calendar = WorkingTime::compile(
        CalendarRules::Weekly(&definition),
        chrono_tz::America::New_York,
        origin(),
        0.0,
        24.0 * 14.0,
    )
    .expect("compiled");
    assert_eq!(
        calendar.add(17.0, 1.0),
        Ok(2.0 * 24.0 + 9.0),
        "Tuesday is closed"
    );
    assert_eq!(
        calendar.add(4.0 * 24.0 + 17.0, 2.0),
        Ok(5.0 * 24.0 + 12.0),
        "the Saturday shift works 10:00-12:00"
    );
}

#[test]
fn queries_outside_the_window_are_reported() {
    let calendar = standard(48.0);
    assert!(calendar.add(0.0, 100.0).is_err());
    assert!(calendar.sub(17.0, 9.0).is_err());
    assert!(calendar.align(24.0 * 30.0).is_err());
}

#[test]
fn always_counts_every_hour() {
    let calendar = WorkingTime::always();
    assert_eq!(calendar.add(5.0, 3.0), Ok(8.0));
    assert_eq!(calendar.sub(5.0, 7.0), Ok(-2.0));
    assert_eq!(calendar.align(3.5), Ok(3.5));
    assert_eq!(calendar.between(1.0, 4.0), Ok(3.0));
}

#[test]
fn offsets_widen_their_window_until_the_answer_fits() {
    let rules = CalendarRules::Standard;
    let zone = chrono_tz::America::New_York;
    assert_eq!(
        WorkingTime::offset_after(rules, zone, origin(), 8.0),
        Some(17.0)
    );
    let long = WorkingTime::offset_after(rules, zone, origin(), 2000.0).expect("fits");
    assert!(
        long > 24.0 * 7.0 * 49.0,
        "2000 working hours take about a year: {long}"
    );
}
