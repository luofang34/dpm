use crate::*;
use chrono::TimeZone;

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn with_zone() -> Plan {
    let mut plan = fixture();
    plan.calendars = Some(Calendars::in_zone("Europe/Berlin"));
    plan
}

fn task(plan: &Plan, key: &str) -> WorkItem {
    plan.find_work_by_key(key).expect("task").clone()
}

#[test]
fn a_time_zone_alone_gives_microsoft_project_defaults() {
    let parsed: Calendars =
        serde_json::from_str(r#"{"time_zone": "Europe/Berlin"}"#).expect("minimal calendars");
    assert_eq!(parsed, Calendars::in_zone("Europe/Berlin"));
    assert_eq!(parsed.kinds.human, STANDARD);
    assert_eq!(parsed.kinds.agent, ALWAYS);
    assert_eq!(parsed.default_executor, ActorKind::Human);
    assert_eq!(parsed.verifier, ActorKind::Human);
    assert_eq!(
        serde_json::to_value(&parsed).expect("serialize"),
        serde_json::json!({"time_zone": "Europe/Berlin"}),
        "defaults are omitted, so the minimal form round-trips"
    );
}

#[test]
fn plans_without_calendars_serialize_as_before() {
    let plan = fixture();
    assert!(plan.calendars.is_none());
    let value = serde_json::to_value(&plan).expect("serialize");
    assert!(value.get("calendars").is_none());
    for work in value["work_items"].as_object().expect("work").values() {
        let fields: Vec<_> = work["schedule"]
            .as_object()
            .expect("schedule")
            .keys()
            .collect();
        assert_eq!(fields, ["estimate", "priority"]);
    }
    for edge in value["dependencies"].as_array().expect("edges") {
        assert!(edge.get("lag_basis").is_none());
    }
    assert_eq!(
        serde_json::from_value::<Plan>(value).expect("reparse"),
        plan
    );
}

#[test]
fn working_periods_read_and_write_clock_times() {
    let period: WorkingPeriod =
        serde_json::from_str(r#"{"from": "08:30", "to": "24:00"}"#).expect("period");
    assert_eq!(period.start().minutes(), 8 * 60 + 30);
    assert_eq!(period.end().minutes(), 24 * 60);
    assert_eq!(
        serde_json::to_string(&period).expect("serialize"),
        r#"{"from":"08:30","to":"24:00"}"#
    );
    for bad in ["24:01", "8:00", "12:60", "noon", "-1:00"] {
        assert!(bad.parse::<ClockTime>().is_err(), "{bad} must be rejected");
    }
}

#[test]
fn calendars_resolve_task_then_actor_then_kind() {
    let mut plan = with_zone();
    let calendars = plan.calendars.as_mut().expect("calendars");
    calendars.default_executor = ActorKind::Agent;
    calendars
        .actors
        .insert("human:ada".into(), "evenings".into());
    calendars.definitions.insert(
        "evenings".into(),
        CalendarDefinition {
            week: WeekPattern {
                sat: vec![WorkingPeriod::new(
                    ClockTime::hours(10),
                    ClockTime::hours(22),
                )],
                ..WeekPattern::default()
            },
            exceptions: Vec::new(),
        },
    );
    plan.validate().expect("valid calendars");
    let calendars = plan.calendars.clone().expect("calendars");
    let mut work = task(&plan, "TEST-A");
    let resolved = calendars.resolve(&work);
    assert_eq!(
        (
            resolved.calendar.as_str(),
            resolved.executor,
            resolved.source
        ),
        (ALWAYS, ActorKind::Agent, CalendarSource::Kind),
        "unowned work without an executor uses the default executor"
    );
    work.schedule.executor = Some(ActorKind::Human);
    assert_eq!(calendars.resolve(&work).calendar, STANDARD);
    work.execution.owner = Some(ActorId::human("ada"));
    let resolved = calendars.resolve(&work);
    assert_eq!(
        (resolved.calendar.as_str(), resolved.source),
        ("evenings", CalendarSource::Actor)
    );
    work.execution.owner = Some(ActorId::agent("builder"));
    assert_eq!(
        calendars.resolve(&work).executor,
        ActorKind::Agent,
        "the owner's kind wins"
    );
    work.schedule.calendar = Some(STANDARD.into());
    let resolved = calendars.resolve(&work);
    assert_eq!(
        (resolved.calendar.as_str(), resolved.source),
        (STANDARD, CalendarSource::Task)
    );
}

type Corruption = Box<dyn Fn(&mut Plan)>;

#[test]
fn invalid_calendars_are_rejected() {
    let period =
        |from: u16, to: u16| WorkingPeriod::new(ClockTime::hours(from), ClockTime::hours(to));
    let day = |periods: Vec<WorkingPeriod>| CalendarDefinition {
        week: WeekPattern {
            mon: periods,
            ..WeekPattern::default()
        },
        exceptions: Vec::new(),
    };
    let cases: Vec<Corruption> = vec![
        Box::new(|p| cal(p).time_zone = "Mars/Olympus".into()),
        Box::new(|p| cal(p).kinds.human = "missing".into()),
        Box::new(|p| {
            cal(p).actors.insert("ada".into(), STANDARD.into());
        }),
        Box::new(|p| {
            cal(p).actors.insert("human:ada".into(), "missing".into());
        }),
        Box::new(move |p| {
            cal(p)
                .definitions
                .insert("x".into(), day(vec![period(9, 12), period(11, 13)]));
        }),
        Box::new(move |p| {
            cal(p)
                .definitions
                .insert("x".into(), day(vec![period(12, 9)]));
        }),
        Box::new(|p| {
            cal(p)
                .definitions
                .insert("empty".into(), CalendarDefinition::default());
        }),
        Box::new(move |p| {
            cal(p)
                .definitions
                .insert(ALWAYS.into(), day(vec![period(9, 17)]));
        }),
        Box::new(move |p| {
            let mut definition = day(vec![period(9, 17)]);
            let date = chrono::NaiveDate::from_ymd_opt(2026, 12, 24).expect("date");
            for (from, to) in [
                (date, date.succ_opt()),
                (date.succ_opt().expect("next"), None),
            ] {
                definition.exceptions.push(CalendarException {
                    from,
                    to,
                    hours: Vec::new(),
                    name: None,
                });
            }
            cal(p).definitions.insert("x".into(), definition);
        }),
        Box::new(|p| {
            p.find_work_by_key_mut("TEST-A")
                .expect("task")
                .schedule
                .calendar = Some("missing".into());
        }),
    ];
    for (index, corrupt) in cases.iter().enumerate() {
        let mut plan = with_zone();
        corrupt(&mut plan);
        assert!(plan.validate().is_err(), "case {index} must be rejected");
    }
    let mut plan = fixture();
    plan.find_work_by_key_mut("TEST-A")
        .expect("task")
        .schedule
        .calendar = Some(ALWAYS.into());
    assert!(
        plan.validate().is_err(),
        "a task calendar needs a calendars block"
    );
}

fn cal(plan: &mut Plan) -> &mut Calendars {
    plan.calendars.as_mut().expect("calendars")
}

#[test]
fn working_lags_count_the_successors_calendar() {
    let mut plan = with_zone();
    let edge = plan.dependencies.first().cloned().expect("edge");
    let friday = chrono_tz::Europe::Berlin
        .with_ymd_and_hms(2026, 3, 6, 16, 0, 0)
        .single()
        .expect("friday")
        .with_timezone(&chrono::Utc);
    let elapsed = plan.lag_end(&edge, friday, 2.0).expect("elapsed");
    assert_eq!(elapsed - friday, chrono::TimeDelta::hours(2));
    let mut working = edge.clone();
    working.lag_basis = LagBasis::Working;
    plan.find_work_by_key_mut("TEST-B")
        .expect("successor")
        .schedule
        .executor = Some(ActorKind::Human);
    let monday = plan.lag_end(&working, friday, 2.0).expect("working");
    assert_eq!(
        monday
            .with_timezone(&chrono_tz::Europe::Berlin)
            .to_rfc3339(),
        "2026-03-09T09:00:00+01:00",
        "one hour on Friday and one on Monday"
    );
    plan.calendars = None;
    assert_eq!(
        plan.lag_end(&working, friday, 2.0),
        Some(elapsed),
        "without calendars a working lag is elapsed"
    );
}

#[test]
fn closures_up_to_five_years_are_valid() {
    let mut plan = with_zone();
    let mut definition = standard_definition();
    definition.exceptions.push(CalendarException {
        from: chrono::NaiveDate::from_ymd_opt(2026, 3, 2).expect("date"),
        to: chrono::NaiveDate::from_ymd_opt(2030, 3, 2),
        hours: Vec::new(),
        name: Some("sabbatical".into()),
    });
    cal(&mut plan).definitions.insert("away".into(), definition);
    plan.validate().expect("a four-year closure is valid");
}

#[test]
fn closures_longer_than_five_years_are_rejected() {
    let closure = |from: (i32, u32, u32), to: (i32, u32, u32)| CalendarException {
        from: chrono::NaiveDate::from_ymd_opt(from.0, from.1, from.2).expect("date"),
        to: chrono::NaiveDate::from_ymd_opt(to.0, to.1, to.2),
        hours: Vec::new(),
        name: Some("until further notice".into()),
    };
    let mut plan = with_zone();
    let mut definition = standard_definition();
    // Two closures a weekend apart form one six-year stretch.
    definition
        .exceptions
        .push(closure((2026, 3, 2), (2029, 3, 2)));
    definition
        .exceptions
        .push(closure((2029, 3, 5), (2032, 3, 5)));
    cal(&mut plan)
        .definitions
        .insert("closed".into(), definition);
    assert!(plan.validate().is_err());
}
