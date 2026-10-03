//! The schedule projection of conditional work: open choices claim no forecast, and a resolved
//! choice schedules only the selected branch.
#![cfg(test)]

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{
    Command, ScheduleProjection, apply_command, schedule_projection, schedule_span, status,
};
use dpm_model::{ActorId, Applicability, Key, Plan, Timeline, WorkItemId};
use dpm_schedule::{SimulationConfig, deterministic_remaining, simulate_remaining};

fn fixture() -> Plan {
    serde_json::from_str(include_str!("../../../tests/support/conditional-plan.json"))
        .expect("conditional fixture")
}

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

fn id(plan: &Plan, key: &str) -> WorkItemId {
    plan.find_work_by_key(key).expect("key").id
}

fn decide(plan: &mut Plan, option: &str, at: DateTime<Utc>) {
    let decision = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let command = Command::Decide {
        decision,
        outcome: option.into(),
    };
    apply_command(
        plan,
        ActorId::human("lead"),
        command,
        at,
        dpm_model::OperationId::new(),
    )
    .expect("decide");
}

fn complete(plan: &mut Plan, key: &str, at: DateTime<Utc>) {
    let work = id(plan, key);
    let worker = ActorId::agent("worker");
    for command in [
        Command::Claim { work },
        Command::Start {
            work,
            occurred_at: None,
        },
        Command::Submit {
            work,
            note: None,
            occurred_at: None,
        },
    ] {
        apply_command(
            plan,
            worker.clone(),
            command,
            at,
            dpm_model::OperationId::new(),
        )
        .expect("execute");
    }
    let verify = Command::Verify {
        work,
        note: None,
        occurred_at: None,
    };
    apply_command(
        plan,
        ActorId::human("lead"),
        verify,
        at,
        dpm_model::OperationId::new(),
    )
    .expect("verify");
}

fn row<'a>(projection: &'a ScheduleProjection, key: &str) -> &'a dpm_engine::ScheduledWork {
    projection
        .work
        .iter()
        .find(|w| w.key.0 == key)
        .expect("row")
}

fn assert_unplaced(row: &dpm_engine::ScheduledWork) {
    let key = &row.key;
    assert!(row.span.is_none(), "{key} span");
    assert!(row.times.is_none(), "{key} times");
    assert!(row.criticality.is_none(), "{key} criticality");
    assert!(row.calendar.is_none(), "{key} calendar");
}

#[test]
fn an_open_choice_has_no_percentiles_and_no_criticality_anywhere() {
    let mut plan = fixture();
    complete(&mut plan, "SUP-DESIGN", t(1));
    let at = t(2);
    let projection = schedule_projection(&plan, true, at).expect("projection");
    assert!(projection.uncertainty.is_none());
    assert!(!projection.work.is_empty());
    for work in &projection.work {
        assert_eq!(work.criticality, None, "{} never reads zero", work.key);
    }
    let summary = status(&plan, true, at).expect("status");
    assert_eq!(summary.p50_finish_hours, None);
    assert_eq!(summary.p80_finish_hours, None);
    assert_eq!(summary.p95_finish_hours, None);
    assert!(summary.open_choices.is_some());

    let mut unplaced = 0;
    for work in &projection.work {
        match work.applicability {
            Applicability::Applicable => {
                assert!(work.times.is_some(), "{} keeps its times", work.key);
            }
            Applicability::Undecided { .. } | Applicability::AwaitingChoice { .. } => {
                unplaced += 1;
                assert_unplaced(work);
            }
            _ => assert_unplaced(work),
        }
    }
    assert!(unplaced > 0, "some work waits for the choice");
    assert!(
        projection
            .work
            .iter()
            .any(|w| matches!(w.applicability, Applicability::Undecided { .. }))
    );
    assert!(row(&projection, "SUP-DESIGN").times.is_some());
}

fn assert_package_covers_its_applicable_children(projection: &ScheduleProjection, key: &str) {
    let package = row(projection, key);
    assert!(package.applicability.is_applicable());
    let children: Vec<_> = projection
        .work
        .iter()
        .filter(|w| w.parent == Some(package.id) && w.applicability.is_applicable())
        .collect();
    assert!(!children.is_empty());
    let span = package.span.expect("selected package span");
    for child in &children {
        let child_span = child.span.expect("child span");
        assert!(span.start_hours <= child_span.start_hours);
        assert!(span.finish_hours >= child_span.finish_hours);
    }
    assert!(
        children
            .iter()
            .any(|c| c.span.is_some_and(|s| s.start_hours == span.start_hours))
    );
    assert!(
        children
            .iter()
            .any(|c| c.span.is_some_and(|s| s.finish_hours == span.finish_hours))
    );
}

#[test]
fn a_resolved_choice_forecasts_and_places_only_the_selected_branch() {
    let mut open = fixture();
    complete(&mut open, "SUP-DESIGN", t(1));
    let at = t(3);
    for (option, selected, unselected) in [
        ("A", "SUP-PKG-A", "SUP-PKG-B"),
        ("B", "SUP-PKG-B", "SUP-PKG-A"),
    ] {
        let mut plan = open.clone();
        decide(&mut plan, option, t(2));
        let projection = schedule_projection(&plan, true, at).expect("projection");
        let summary = status(&plan, true, at).expect("status");
        let config = SimulationConfig {
            iterations: 2000,
            ..SimulationConfig::default()
        };
        let simulation = simulate_remaining(&plan, config, at).expect("simulation");
        let deterministic = deterministic_remaining(&plan, at).expect("cpm");

        let uncertainty = projection.uncertainty.as_ref().expect("uncertainty");
        assert_eq!(Some(uncertainty.p50_finish_hours), summary.p50_finish_hours);
        assert_eq!(Some(uncertainty.p80_finish_hours), summary.p80_finish_hours);
        assert_eq!(Some(uncertainty.p95_finish_hours), summary.p95_finish_hours);
        assert_eq!(uncertainty.iterations, 2000);
        assert_eq!(uncertainty.iterations, simulation.iterations);
        assert_eq!(uncertainty.seed, config.seed);
        assert_eq!(
            projection.project_finish_hours, deterministic.project_finish_hours,
            "option {option}"
        );

        let mut applicable = 0;
        let mut not_selected = 0;
        for work in &projection.work {
            let expected = simulation.criticality.get(&work.id).copied();
            if work.applicability.is_applicable() {
                applicable += 1;
                assert!(work.times.is_some(), "{option}: {} times", work.key);
                assert!(work.span.is_some(), "{option}: {} span", work.key);
                assert!(expected.is_some(), "{option}: {} simulated", work.key);
                assert_eq!(work.criticality, expected, "{option}: {}", work.key);
            } else {
                if matches!(work.applicability, Applicability::NotSelected { .. }) {
                    not_selected += 1;
                }
                assert_unplaced(work);
                assert!(expected.is_none(), "{option}: {} not simulated", work.key);
            }
        }
        assert!(applicable > 0);
        assert!(not_selected > 0, "option {option} leaves work unselected");

        assert_package_covers_its_applicable_children(&projection, selected);
        let other = row(&projection, unselected);
        assert!(matches!(
            other.applicability,
            Applicability::NotSelected { .. }
        ));
        assert_unplaced(other);
        if option == "B" {
            let audit = row(&projection, "SUP-A-AUDIT");
            assert!(!audit.applicability.is_applicable());
            assert_unplaced(audit);
        }
    }
}

fn add_unconditioned_package_over_a_children(plan: &mut Plan) -> WorkItemId {
    let template = plan.find_work_by_key("SUP-PKG-A").expect("package").clone();
    let condition = template.contract.condition.clone();
    assert!(condition.is_some());
    let quote = plan.find_work_by_key("SUP-A-QUOTE").expect("task").clone();
    let mut package = template;
    package.id = WorkItemId::new();
    package.key = Key::new("SUP-PKG-ALL");
    package.contract.condition = None;
    let package_id = package.id;
    plan.work_items.insert(package_id, package);
    for key in ["SUP-ALL-ONE", "SUP-ALL-TWO"] {
        let mut child = quote.clone();
        child.id = WorkItemId::new();
        child.key = Key::new(key);
        child.parent = Some(package_id);
        child.contract.condition = condition.clone();
        plan.work_items.insert(child.id, child);
    }
    package_id
}

#[test]
fn a_package_without_a_condition_whose_children_are_all_excluded_has_no_span() {
    let mut plan = fixture();
    let package_id = add_unconditioned_package_over_a_children(&mut plan);
    plan.validate().expect("plan stays valid");
    decide(&mut plan, "B", t(2));
    let at = t(3);

    let timeline = Timeline::at(&plan, at);
    assert!(matches!(
        timeline.applicability(package_id),
        Applicability::AllChildrenExcluded { .. }
    ));
    for key in ["SUP-ALL-ONE", "SUP-ALL-TWO"] {
        assert!(matches!(
            timeline.applicability(id(&plan, key)),
            Applicability::NotSelected { .. }
        ));
    }

    let schedule = deterministic_remaining(&plan, at).expect("cpm");
    assert!(!schedule.activities.contains_key(&package_id));
    let package = plan.work_items[&package_id].clone();
    assert!(schedule_span(&plan, &timeline, &schedule, &package).is_none());

    let projection = schedule_projection(&plan, true, at).expect("projection");
    let package_row = row(&projection, "SUP-PKG-ALL");
    assert!(matches!(
        package_row.applicability,
        Applicability::AllChildrenExcluded { .. }
    ));
    assert_unplaced(package_row);
}
