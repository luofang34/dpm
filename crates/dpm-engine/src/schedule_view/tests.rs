use super::*;
use crate::{NextWorkQuery, next_work, status};
use dpm_model::Calendars;
use dpm_schedule::{deterministic_remaining, simulate_remaining};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn now() -> DateTime<Utc> {
    chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 9, 1, 0, 0, 0)
        .single()
        .expect("time")
}

#[test]
fn deterministic_values_equal_the_engine_projection_and_carry_no_uncertainty() {
    let plan = fixture();
    let projection = schedule_projection(&plan, false, now()).expect("projection");
    let schedule = deterministic_remaining(&plan, now()).expect("schedule");
    assert_eq!(
        projection.project_finish_hours,
        schedule.project_finish_hours
    );
    assert!(projection.uncertainty.is_none());
    assert_eq!(projection.work.len(), plan.work_items.len());
    for row in &projection.work {
        assert_eq!(row.criticality, None, "no zero stands in for unknown");
        assert!(row.calendar.is_none(), "no calendars, no placement");
        let activity = schedule.activities.get(&row.id).expect("activity");
        let times = row.times.as_ref().expect("times");
        assert_eq!(times.earliest_start_hours, activity.earliest_start_hours);
        assert_eq!(times.earliest_finish_hours, activity.earliest_finish_hours);
        assert_eq!(times.latest_start_hours, activity.latest_start_hours);
        assert_eq!(times.latest_finish_hours, activity.latest_finish_hours);
        assert_eq!(times.total_float_hours, activity.total_float_hours);
        assert_eq!(times.free_float_hours, activity.free_float_hours);
        assert_eq!(times.critical, activity.critical);
    }
    let keys: Vec<_> = projection.work.iter().map(|w| w.key.clone()).collect();
    let mut sorted = keys.clone();
    sorted.sort_by(|a, b| a.natural_cmp(b));
    assert_eq!(keys, sorted);
}

#[test]
fn probabilistic_values_equal_the_summary_status_and_next_use() {
    let plan = fixture();
    let projection = schedule_projection(&plan, true, now()).expect("projection");
    let summary = simulate_remaining(&plan, crate::query::simulation_config(), now()).expect("sim");
    let uncertainty = projection.uncertainty.expect("uncertainty");
    let status = status(&plan, true, now()).expect("status");
    assert_eq!(Some(uncertainty.p50_finish_hours), status.p50_finish_hours);
    assert_eq!(Some(uncertainty.p80_finish_hours), status.p80_finish_hours);
    assert_eq!(Some(uncertainty.p95_finish_hours), status.p95_finish_hours);
    assert_eq!(uncertainty.iterations, summary.iterations);
    assert_eq!(uncertainty.seed, crate::query::simulation_config().seed);
    for row in &projection.work {
        assert_eq!(row.criticality, summary.criticality.get(&row.id).copied());
    }
    for candidate in next_work(&plan, &NextWorkQuery::default(), now()).expect("next") {
        let row = projection
            .work
            .iter()
            .find(|w| w.id == candidate.work.id)
            .expect("row");
        assert_eq!(row.criticality, candidate.criticality);
    }
}

#[test]
fn a_calendar_plan_places_work_and_a_plain_plan_does_not() {
    let mut plan = fixture();
    plan.calendars = Some(Calendars {
        time_zone: "UTC".into(),
        definitions: Default::default(),
        kinds: Default::default(),
        actors: Default::default(),
        default_executor: dpm_model::ActorKind::Human,
        verifier: dpm_model::ActorKind::Human,
    });
    let projection = schedule_projection(&plan, false, now()).expect("projection");
    assert!(projection.work.iter().any(|w| w.calendar.is_some()));
    let plain = schedule_projection(&fixture(), false, now()).expect("plain");
    assert!(plain.work.iter().all(|w| w.calendar.is_none()));
}

/// `count` copies of TEST-A in one project, flat or in one dependency chain.
fn generated(count: usize, chained: bool) -> Plan {
    let mut plan = fixture();
    let template = plan.work_items[&plan.find_work_by_key("TEST-A").expect("a").id].clone();
    plan.work_items.clear();
    plan.dependencies.clear();
    plan.decisions.clear();
    plan.risks.clear();
    let mut previous = None;
    for index in 0..count {
        let mut item = template.clone();
        item.id = WorkItemId::new();
        item.key = Key::new(format!("GEN-{index}"));
        item.parent = None;
        item.contract.condition = None;
        if let (true, Some(before)) = (chained, previous) {
            plan.dependencies.push(dpm_model::Dependency::new(
                before,
                item.id,
                dpm_model::DependencyKind::FinishStart,
                0.0,
            ));
        }
        previous = Some(item.id);
        plan.work_items.insert(item.id, item);
    }
    plan
}

#[test]
fn the_5000_task_flat_and_chain_shapes_project_every_item() {
    for chained in [false, true] {
        let plan = generated(5000, chained);
        let projection = schedule_projection(&plan, false, now()).expect("projection");
        let schedule = deterministic_remaining(&plan, now()).expect("schedule");
        assert_eq!(projection.work.len(), 5000);
        assert_eq!(
            projection.project_finish_hours,
            schedule.project_finish_hours
        );
        assert!(
            projection
                .work
                .iter()
                .all(|w| w.times.is_some() && w.criticality.is_none())
        );
        let critical = projection
            .work
            .iter()
            .filter(|w| w.span.is_some_and(|s| s.critical));
        // Equal tasks are all critical, side by side or in a chain.
        assert_eq!(critical.count(), 5000, "chained: {chained}");
    }
}

#[test]
fn an_empty_plan_has_no_rows_and_no_percentiles() {
    let plan = Plan::empty("empty");
    let projection = schedule_projection(&plan, true, now()).expect("projection");
    assert!(projection.work.is_empty());
    assert!(projection.uncertainty.is_none());
    assert_eq!(projection.project_finish_hours, 0.0);
}

#[test]
fn a_package_spans_its_only_child() {
    let mut plan = fixture();
    let child = plan.find_work_by_key("TEST-A").expect("child").id;
    let mut package = plan.work_items[&child].clone();
    package.id = WorkItemId::new();
    package.key = Key::new("WP");
    package.kind = WorkKind::WorkPackage;
    package.schedule.estimate = None;
    let id = package.id;
    plan.work_items.insert(id, package);
    plan.work_items.get_mut(&child).expect("child").parent = Some(id);
    let projection = schedule_projection(&plan, false, now()).expect("projection");
    let row = |id| projection.work.iter().find(|w| w.id == id).expect("row");
    assert_eq!(row(id).span, row(child).span);
    assert!(row(id).span.is_some());
}

/// An empty work package under `parent`, built from an existing item.
fn add_package(plan: &mut Plan, key: &str, parent: Option<WorkItemId>) -> WorkItemId {
    let mut package = plan.work_items.values().next().expect("template").clone();
    package.id = WorkItemId::new();
    package.key = Key::new(key);
    package.kind = WorkKind::WorkPackage;
    package.parent = parent;
    package.schedule.estimate = None;
    package.contract.condition = None;
    let id = package.id;
    plan.work_items.insert(id, package);
    id
}

/// A task of `hours` under `parent`, starting after `after` finishes.
fn add_task(
    plan: &mut Plan,
    key: &str,
    hours: f64,
    parent: Option<WorkItemId>,
    after: Option<WorkItemId>,
) -> WorkItemId {
    let mut item = plan
        .work_items
        .values()
        .find(|w| w.kind != WorkKind::WorkPackage)
        .expect("template")
        .clone();
    item.id = WorkItemId::new();
    item.key = Key::new(key);
    item.parent = parent;
    item.contract.condition = None;
    let estimate = item.schedule.estimate.as_mut().expect("estimate");
    estimate.optimistic_hours = hours;
    estimate.likely_hours = hours;
    estimate.pessimistic_hours = hours;
    let id = item.id;
    if let Some(before) = after {
        plan.dependencies.push(dpm_model::Dependency::new(
            before,
            id,
            dpm_model::DependencyKind::FinishStart,
            0.0,
        ));
    }
    plan.work_items.insert(id, item);
    id
}

#[test]
fn an_empty_package_spans_zero_and_is_not_critical() {
    let mut plan = generated(1, false);
    let empty = add_package(&mut plan, "EMPTY", None);
    let timeline = Timeline::at(&plan, now());
    let schedule = deterministic_remaining_at(&plan, &timeline).expect("schedule");
    let expected = ScheduleSpan {
        start_hours: 0.0,
        finish_hours: 0.0,
        critical: false,
    };
    assert_eq!(
        schedule_span(&plan, &timeline, &schedule, &plan.work_items[&empty]),
        Some(expected)
    );
    let projection = schedule_projection(&plan, false, now()).expect("projection");
    let row = projection.work.iter().find(|w| w.id == empty).expect("row");
    assert_eq!(row.span, Some(expected));
    // An applicable package is a zero-duration activity of the schedule, not the span's source.
    let times = row.times.as_ref().expect("applicable rows keep times");
    assert_eq!(times.earliest_start_hours, times.earliest_finish_hours);
    assert!(!times.critical);
    assert_eq!(row.criticality, None);
}

#[test]
fn nested_packages_union_their_descendants_and_critical_is_an_or() {
    let mut plan = generated(1, false);
    let outer = add_package(&mut plan, "OUTER", None);
    let inner = add_package(&mut plan, "INNER", Some(outer));
    let first = add_task(&mut plan, "FIRST", 4.0, Some(outer), None);
    // The critical chain continues inside the nested package; the short task floats.
    let second = add_task(&mut plan, "SECOND", 6.0, Some(inner), Some(first));
    let short = add_task(&mut plan, "SHORT", 1.0, Some(inner), Some(first));
    let quiet = add_package(&mut plan, "QUIET", None);
    let quiet_inner = add_package(&mut plan, "QUIET-INNER", Some(quiet));
    let float = add_task(&mut plan, "FLOAT", 1.0, Some(quiet_inner), None);
    let timeline = Timeline::at(&plan, now());
    let schedule = deterministic_remaining_at(&plan, &timeline).expect("schedule");
    let activity = |id: &WorkItemId| schedule.activities.get(id).expect("activity");
    assert!(activity(&first).critical && activity(&second).critical);
    assert!(!activity(&short).critical && !activity(&float).critical);
    assert!(activity(&second).earliest_start_hours > 0.0);

    let outer_span = ScheduleSpan {
        start_hours: activity(&first).earliest_start_hours,
        finish_hours: activity(&second).earliest_finish_hours,
        critical: true,
    };
    let inner_span = ScheduleSpan {
        start_hours: activity(&second).earliest_start_hours,
        finish_hours: activity(&second).earliest_finish_hours,
        critical: true,
    };
    let quiet_span = ScheduleSpan {
        start_hours: activity(&float).earliest_start_hours,
        finish_hours: activity(&float).earliest_finish_hours,
        critical: false,
    };
    assert!(outer_span.start_hours < inner_span.start_hours);
    assert!(outer_span.finish_hours >= activity(&short).earliest_finish_hours);
    let projection = schedule_projection(&plan, false, now()).expect("projection");
    for (id, expected) in [
        (outer, outer_span),
        (inner, inner_span),
        (quiet, quiet_span),
        (quiet_inner, quiet_span),
    ] {
        let item = &plan.work_items[&id];
        assert_eq!(
            schedule_span(&plan, &timeline, &schedule, item),
            Some(expected),
            "{}",
            item.key
        );
        let row = projection.work.iter().find(|w| w.id == id).expect("row");
        assert_eq!(row.span, Some(expected), "row {}", item.key);
    }
}

#[test]
fn a_package_reports_its_descendants_bounds_and_least_float() {
    let mut plan = generated(1, false);
    let package = add_package(&mut plan, "PKG", None);
    let first = add_task(&mut plan, "FIRST", 4.0, Some(package), None);
    let long = add_task(&mut plan, "LONG", 6.0, Some(package), Some(first));
    let short = add_task(&mut plan, "SHORT", 1.0, Some(package), Some(first));
    let quiet = add_package(&mut plan, "QUIET", None);
    let floating = add_task(&mut plan, "FLOAT", 1.0, Some(quiet), None);
    let projection = schedule_projection(&plan, true, now()).expect("projection");
    let row = |id| projection.work.iter().find(|w| w.id == id).expect("row");
    let times = |id| row(id).times.clone().expect("times");
    let rolled = times(package);
    assert_eq!(
        rolled.earliest_start_hours,
        times(first).earliest_start_hours
    );
    assert_eq!(
        rolled.earliest_finish_hours,
        times(long).earliest_finish_hours
    );
    assert_eq!(rolled.latest_start_hours, times(first).latest_start_hours);
    assert_eq!(rolled.latest_finish_hours, times(long).latest_finish_hours);
    assert_eq!(rolled.total_float_hours, 0.0);
    assert!(rolled.critical && times(short).total_float_hours > 0.0);
    assert_eq!(row(package).criticality, Some(1.0));

    // A package whose only work floats keeps that float, not the whole project's duration.
    let quiet_times = times(quiet);
    assert_eq!(
        quiet_times.total_float_hours,
        times(floating).total_float_hours
    );
    assert!(quiet_times.total_float_hours < projection.project_finish_hours);
    assert!(!quiet_times.critical);
    assert_eq!(row(quiet).criticality, row(floating).criticality);
    assert_eq!(row(quiet).span.map(|s| s.critical), Some(false));
}
