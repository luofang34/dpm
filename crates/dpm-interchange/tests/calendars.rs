//! Working calendars survive a Microsoft Project XML round trip through the public API.
#![cfg(test)]

use chrono::NaiveDate;
use dpm_interchange::{ImportOptions, export_mspdi, import_mspdi};
use dpm_model::{
    ALWAYS, ActorKind, CalendarDefinition, CalendarException, Calendars, ClockTime, LagBasis, Plan,
    WeekPattern, WorkingPeriod,
};

fn night() -> CalendarDefinition {
    let shift = vec![
        WorkingPeriod::new(ClockTime::hours(0), ClockTime::hours(2)),
        WorkingPeriod::new(ClockTime::hours(22), ClockTime::hours(24)),
    ];
    CalendarDefinition {
        week: WeekPattern {
            mon: shift.clone(),
            tue: shift.clone(),
            wed: shift.clone(),
            thu: shift,
            ..WeekPattern::default()
        },
        exceptions: vec![CalendarException {
            from: NaiveDate::from_ymd_opt(2026, 12, 24).expect("date"),
            to: NaiveDate::from_ymd_opt(2026, 12, 26),
            hours: Vec::new(),
            name: Some("Holidays & rest".into()),
        }],
    }
}

/// The lifecycle fixture on calendars: A on the human Standard calendar, B on a night shift, C
/// on `always`, D done by an agent; A->B lags 8 working hours, B->C 2 elapsed hours.
fn plan() -> Plan {
    let mut plan: Plan =
        serde_json::from_str(include_str!("../../../tests/support/execution-plan.json"))
            .expect("plan");
    let mut calendars = Calendars::in_zone("Europe/Berlin");
    calendars.definitions.insert("Night".into(), night());
    plan.calendars = Some(calendars);
    let id = |plan: &Plan, key: &str| plan.find_work_by_key(key).expect(key).id;
    for (key, calendar) in [("TEST-B", "Night"), ("TEST-C", ALWAYS)] {
        let work = id(&plan, key);
        plan.work_items.get_mut(&work).expect(key).schedule.calendar = Some(calendar.into());
    }
    let d = id(&plan, "TEST-D");
    plan.work_items.get_mut(&d).expect("D").schedule.executor = Some(ActorKind::Agent);
    let (a, b, c) = (
        id(&plan, "TEST-A"),
        id(&plan, "TEST-B"),
        id(&plan, "TEST-C"),
    );
    for edge in &mut plan.dependencies {
        match (edge.predecessor, edge.successor) {
            (p, s) if (p, s) == (a, b) => {
                (edge.lag_hours, edge.lag_basis) = (8.0, LagBasis::Working)
            }
            (p, s) if (p, s) == (b, c) => {
                (edge.lag_hours, edge.lag_basis) = (2.0, LagBasis::Elapsed)
            }
            (p, _) if p == a => edge.lag_basis = LagBasis::Working,
            _ => {}
        }
    }
    plan.validate().expect("valid plan");
    plan
}

fn options(time_zone: Option<&str>) -> ImportOptions {
    ImportOptions {
        project_key: "TEST".into(),
        key_prefix: None,
        match_existing_by: None,
        keep_existing_priority: false,
        time_zone: time_zone.map(str::to_owned),
    }
}

fn task_xml<'a>(xml: &'a str, title: &str) -> &'a str {
    let name = format!("<Name>{title}</Name>");
    xml.split("<Task>")
        .find(|t| t.contains(&name))
        .expect(title)
}

#[test]
fn export_writes_used_calendars_task_calendars_and_time_formats() {
    let plan = plan();
    let exported = export_mspdi(&plan, "TEST").expect("export");
    let xml = &exported.xml;
    assert!(xml.contains("<CalendarUID>1</CalendarUID>\n    <ExtendedAttributes>"));
    for name in ["Standard", "Night", "24 Hours"] {
        assert!(xml.contains(&format!(
            "<Name>{name}</Name>\n            <IsBaseCalendar>1"
        )));
    }
    assert!(xml.contains("<Name>Holidays &amp; rest</Name>"));
    assert!(xml.contains("<FromTime>22:00:00</FromTime><ToTime>00:00:00</ToTime>"));
    let a = task_xml(xml, "Contract A");
    assert!(a.contains("<DurationFormat>5</DurationFormat>") && !a.contains("<CalendarUID>"));
    let b = task_xml(xml, "Contract B");
    assert!(b.contains("<CalendarUID>2</CalendarUID>") && b.contains("<LagFormat>5</LagFormat>"));
    let c = task_xml(xml, "Contract C");
    assert!(
        c.contains("<DurationFormat>6</DurationFormat>") && c.contains("<LagFormat>6</LagFormat>")
    );
    assert!(c.contains("<CalendarUID>3</CalendarUID>"));
    let omitted: Vec<_> = exported
        .report
        .omitted
        .iter()
        .map(|f| f.field.as_str())
        .collect();
    assert!(omitted.contains(&"time_zone") && omitted.contains(&"calendar_kinds"));
}

#[test]
fn reimporting_an_export_changes_nothing_with_or_without_the_time_zone() {
    let plan = plan();
    let xml = export_mspdi(&plan, "TEST").expect("export").xml;
    for zone in [Some("Europe/Berlin"), None] {
        let imported = import_mspdi(&plan, &xml, &options(zone)).expect("import");
        assert_eq!(imported.candidate, plan, "{zone:?}");
        let preview = dpm_engine::propose_change(&plan, &imported.candidate).expect("preview");
        assert!(
            preview.changes.is_empty(),
            "{zone:?}: {:?}",
            preview.changes
        );
        assert!(imported.report.links.iter().all(|l| l.changes.is_empty()));
    }
}

#[test]
fn a_fresh_workspace_imports_calendars_task_calendars_and_lag_bases() {
    let source = plan();
    let xml = export_mspdi(&source, "TEST").expect("export").xml;
    let mut empty = source.clone();
    empty.work_items.clear();
    empty.dependencies.clear();
    empty.calendars = None;
    let imported = import_mspdi(&empty, &xml, &options(Some("Europe/Berlin"))).expect("import");
    let candidate = imported.candidate;
    candidate.validate().expect("valid candidate");
    assert_eq!(candidate.calendars, source.calendars);
    let calendar = |key: &str| {
        let work = candidate.find_work_by_key(key).expect(key);
        candidate.work_calendar(work).expect("calendars").calendar
    };
    for key in ["TEST-A", "TEST-B", "TEST-C", "TEST-D", "TEST-E"] {
        let original = source.find_work_by_key(key).expect(key);
        assert_eq!(
            calendar(key),
            source.work_calendar(original).expect("calendars").calendar,
            "{key}"
        );
    }
    let bases: Vec<_> = candidate
        .dependencies
        .iter()
        .filter(|d| d.lag_hours != 0.0)
        .map(|d| (d.lag_hours, d.lag_basis))
        .collect();
    assert_eq!(bases, [(8.0, LagBasis::Working), (2.0, LagBasis::Elapsed)]);
    assert!(imported.report.items.iter().all(|item| {
        item.approximated
            .iter()
            .all(|f| !f.detail.contains("calendar not applied"))
    }));
}

fn release_workspace() -> Plan {
    let mut plan = Plan::empty("Calendar fixtures");
    let project = dpm_model::Project {
        id: dpm_model::ProjectId(uuid::Uuid::from_u128(
            0x51c3_77aa_0b1e_4d0c_9a44_6e2f_8d13_c0de,
        )),
        key: dpm_model::Key::new("REL"),
        parent: None,
        title: "Release".into(),
        objective: "Ship the release".into(),
    };
    plan.projects.insert(project.id, project);
    plan
}

#[test]
fn tool_fixtures_schedule_on_their_standard_calendar_given_a_time_zone() {
    let fixtures = [
        (include_str!("mspdi/mpxj-release-plan.xml"), None),
        (include_str!("mspdi/omniplan-native.xml"), Some("OMNI")),
    ];
    for (xml, prefix) in fixtures {
        let options = ImportOptions {
            project_key: "REL".into(),
            key_prefix: prefix.map(str::to_owned),
            ..options(Some("America/New_York"))
        };
        let plan = release_workspace();
        let imported = import_mspdi(&plan, xml, &options).expect("import");
        let candidate = &imported.candidate;
        dpm_engine::propose_change(&plan, candidate).expect("reviewable candidate");
        let calendars = candidate.calendars.as_ref().expect("calendars");
        assert!(calendars.definitions.is_empty() && calendars.kinds.human == "standard");
        let report = &imported.report;
        assert!(report.rejected.iter().all(|f| f.field != "calendars"));
        for link in &report.links {
            assert!(
                link.notes
                    .iter()
                    .all(|n| !n.contains("calendar not applied")),
                "{link:?}"
            );
        }
        for item in &report.items {
            assert!(
                item.approximated
                    .iter()
                    .all(|f| !f.detail.contains("calendar not applied"))
            );
        }
        assert!(
            candidate
                .dependencies
                .iter()
                .any(|d| d.lag_basis == LagBasis::Working)
        );
    }
    let native = import_mspdi(
        &release_workspace(),
        include_str!("mspdi/omniplan-native.xml"),
        &ImportOptions {
            project_key: "REL".into(),
            key_prefix: Some("OMNI".into()),
            ..options(Some("America/New_York"))
        },
    )
    .expect("import");
    let engineer = &native.report.calendars[1];
    assert_eq!(engineer.name, "李工程师");
    assert_eq!(engineer.outcome, dpm_interchange::CalendarOutcome::Skipped);
}

#[test]
fn a_workspace_with_other_kind_calendars_names_the_source_calendar_on_each_task() {
    let mut plan = release_workspace();
    let mut calendars = Calendars::in_zone("America/New_York");
    calendars.kinds.human = ALWAYS.into();
    plan.calendars = Some(calendars.clone());
    let xml = include_str!("mspdi/mpxj-release-plan.xml").replace(
        "</Tasks>",
        "<Task><UID>90</UID><Name>Parked</Name><OutlineLevel>1</OutlineLevel><Active>0</Active><CalendarUID>1</CalendarUID></Task></Tasks>",
    );
    let options = ImportOptions {
        project_key: "REL".into(),
        ..options(Some("America/New_York"))
    };
    let imported = import_mspdi(&plan, &xml, &options).expect("import");
    let candidate = &imported.candidate;
    candidate.validate().expect("valid candidate");
    assert_eq!(candidate.calendars.as_ref(), Some(&calendars));
    for work in candidate.work_items.values() {
        let resolved = candidate.work_calendar(work).expect("calendars").calendar;
        let explicit = work.schedule.calendar.as_deref();
        match work.kind {
            dpm_model::WorkKind::WorkPackage => assert_eq!(explicit, None),
            _ if resolved == ALWAYS => assert_eq!(explicit, None, "{}", work.key),
            _ => assert_eq!(explicit, Some("standard"), "{}", work.key),
        }
    }
    assert!(
        candidate
            .work_items
            .values()
            .any(|w| w.schedule.calendar.as_deref() == Some("standard"))
    );
    let parked = imported.report.items.iter().find(|i| i.uid == 90);
    let parked = parked.expect("parked");
    assert_eq!(parked.outcome, dpm_interchange::ItemOutcome::Skipped);
    assert!(parked.rejected.iter().all(|f| f.field != "calendar"));
}

#[test]
fn a_workspace_with_calendars_imports_in_its_own_zone_without_one_given() {
    let source = plan();
    let xml = export_mspdi(&source, "TEST").expect("export").xml;
    let mut workspace = source.clone();
    workspace.work_items.clear();
    workspace.dependencies.clear();
    let imported = import_mspdi(&workspace, &xml, &options(None)).expect("import");
    let candidate = imported.candidate;
    let calendar = |key: &str| {
        let work = candidate.find_work_by_key(key).expect(key);
        candidate.work_calendar(work).expect("calendars").calendar
    };
    assert_eq!(calendar("TEST-C"), ALWAYS, "elapsed durations stay elapsed");
    assert_eq!(calendar("TEST-B"), "Night");
    let bases: Vec<_> = candidate
        .dependencies
        .iter()
        .filter(|d| d.lag_hours != 0.0)
        .map(|d| (d.lag_hours, d.lag_basis))
        .collect();
    assert_eq!(bases, [(8.0, LagBasis::Working), (2.0, LagBasis::Elapsed)]);
}

#[test]
fn updating_a_calendar_names_the_existing_work_that_follows_it() {
    let source = plan();
    let xml = export_mspdi(&source, "TEST").expect("export").xml;
    let mut workspace = source.clone();
    if let Some(calendars) = workspace.calendars.as_mut() {
        calendars.definitions.insert(
            "Night".into(),
            CalendarDefinition {
                week: WeekPattern {
                    fri: vec![WorkingPeriod::new(
                        ClockTime::hours(20),
                        ClockTime::hours(24),
                    )],
                    ..WeekPattern::default()
                },
                exceptions: Vec::new(),
            },
        );
    }
    let imported = import_mspdi(&workspace, &xml, &options(None)).expect("import");
    let night = imported
        .report
        .calendars
        .iter()
        .find(|c| c.calendar.as_deref() == Some("Night"))
        .expect("night report");
    assert_eq!(night.outcome, dpm_interchange::CalendarOutcome::Updated);
    let affected: Vec<_> = night.affected_work.iter().map(|k| k.0.as_str()).collect();
    assert_eq!(affected, ["TEST-B"]);
}
