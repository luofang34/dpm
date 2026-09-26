//! Every projection of conditional work reads the same active graph.
#![allow(clippy::expect_used, clippy::panic)]

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{
    Command, NextWorkQuery, apply_command, explain_work, next_work, progress, status,
};
use dpm_model::{ActorId, Applicability, Plan, WorkItemId};
use dpm_schedule::{SimulationConfig, deterministic_remaining, simulate_remaining};
use std::collections::BTreeSet;

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
    apply_command(plan, ActorId::human("lead"), command, at).expect("decide");
}

fn complete(plan: &mut Plan, key: &str, at: DateTime<Utc>) {
    let work = id(plan, key);
    let worker = ActorId::agent("worker");
    for command in [
        Command::Claim { work },
        Command::Start { work },
        Command::Submit { work, note: None },
    ] {
        apply_command(plan, worker.clone(), command, at).expect("execute");
    }
    let verify = Command::Verify { work, note: None };
    apply_command(plan, ActorId::human("lead"), verify, at).expect("verify");
}

fn keys(plan: &Plan, ids: impl IntoIterator<Item = WorkItemId>) -> BTreeSet<String> {
    ids.into_iter()
        .map(|id| plan.work_items[&id].key.0.clone())
        .collect()
}

fn applicable(plan: &Plan, at: DateTime<Utc>) -> BTreeSet<String> {
    plan.work_items
        .keys()
        .filter(|id| {
            explain_work(plan, **id, at)
                .expect("explain")
                .applicability
                .is_applicable()
        })
        .map(|id| plan.work_items[id].key.0.clone())
        .collect()
}

#[test]
fn next_progress_cpm_and_monte_carlo_share_the_active_graph() {
    let mut plan = fixture();
    decide(&mut plan, "B", t(1));
    complete(&mut plan, "SUP-DESIGN", t(2));
    let at = t(3);
    let active = applicable(&plan, at);
    let expected: BTreeSet<String> = [
        "SUP-DESIGN",
        "SUP-PKG-B",
        "SUP-B-QUOTE",
        "SUP-B-QUAL",
        "SUP-MERGE",
        "SUP-BUILD",
    ]
    .map(String::from)
    .into();
    assert_eq!(active, expected);

    let schedule = deterministic_remaining(&plan, at).expect("cpm");
    assert_eq!(keys(&plan, schedule.activities.keys().copied()), active);
    let config = SimulationConfig {
        iterations: 200,
        seed: 7,
    };
    let simulation = simulate_remaining(&plan, config, at).expect("simulation");
    assert_eq!(keys(&plan, simulation.criticality.keys().copied()), active);
    let hours = |h: f64| h + h / 12.0;
    let finish = hours(3.0) + hours(12.0) + hours(5.0);
    assert!((schedule.project_finish_hours - finish).abs() < 1e-9);

    let query = NextWorkQuery {
        use_probabilistic_criticality: false,
        ..NextWorkQuery::default()
    };
    let next = next_work(&plan, &query, at).expect("next");
    let recommended: Vec<_> = next.iter().map(|c| c.work.key.0.as_str()).collect();
    assert_eq!(recommended, ["SUP-B-QUOTE"]);
    let summary = status(&plan, true, at).expect("status");
    assert_eq!(summary.ready, 1);
    assert!(summary.open_choices.is_none());
    assert!(summary.p50_finish_hours.is_some());
    let excluded: Vec<_> = summary
        .not_applicable
        .iter()
        .map(|w| w.key.0.as_str())
        .collect();
    assert_eq!(
        excluded,
        [
            "SUP-A-AUDIT",
            "SUP-A-QUAL",
            "SUP-A-QUOTE",
            "SUP-A-TEST",
            "SUP-PKG-A"
        ]
    );

    // Selected tasks count; supplier A tasks do not. The stranded audit stays outstanding.
    let report = progress(&plan, at).expect("progress");
    assert!((report.overall.percent_complete - 100.0 / 5.0).abs() < 1e-9);
    let design = explain_work(&plan, id(&plan, "SUP-DESIGN"), at).expect("explain");
    assert_eq!(
        design.downstream_count, 4,
        "B quote, B qualification, merge and build; not supplier A work"
    );
    let skipped = explain_work(&plan, id(&plan, "SUP-A-QUOTE"), at).expect("explain");
    assert!(skipped.schedule.is_none() && skipped.criticality.is_none());
    assert!(matches!(
        skipped.applicability,
        Applicability::NotSelected { .. }
    ));
}

#[test]
fn open_choices_report_one_forecast_per_scenario_instead_of_a_blended_percentile() {
    let mut plan = fixture();
    complete(&mut plan, "SUP-DESIGN", t(1));
    let at = t(2);
    let open = status(&plan, true, at).expect("status");
    assert_eq!(open.p50_finish_hours, None, "no mixed-scenario percentile");
    assert_eq!(
        open.expected_finish_hours, 0.0,
        "only committed work remains"
    );
    let choices = open.open_choices.as_ref().expect("open choices");
    assert_eq!(choices.decisions[0].0, "DEC-SUPPLIER");
    assert_eq!(choices.scenario_count, 2);
    for option in ["A", "B"] {
        let scenario = choices
            .scenarios
            .iter()
            .find(|s| s.choices.values().any(|o| o == option))
            .expect("scenario");
        let mut decided = plan.clone();
        decide(&mut decided, option, at);
        let actual = status(&decided, true, at).expect("status");
        assert_eq!(scenario.expected_finish_hours, actual.expected_finish_hours);
        assert_eq!(scenario.p50_finish_hours, actual.p50_finish_hours);
        assert_eq!(scenario.p95_finish_hours, actual.p95_finish_hours);
        let stranded: Vec<_> = scenario.stranded.iter().map(|k| k.0.as_str()).collect();
        let expected: &[&str] = if option == "B" { &["SUP-A-AUDIT"] } else { &[] };
        assert_eq!(stranded, expected);
    }
    let summary = serde_json::to_value(&open).expect("json");
    assert!(
        summary["not_applicable"]
            .as_array()
            .is_some_and(|a| a.len() == 10)
    );
}

#[test]
fn too_many_open_combinations_are_counted_but_not_forecast() {
    let mut plan = fixture();
    let template = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .clone();
    let tasks = ["SUP-DESIGN", "SUP-BUILD", "SUP-A-AUDIT", "SUP-MERGE"].map(|key| id(&plan, key));
    for (index, work) in tasks.into_iter().enumerate() {
        let mut decision = template.clone();
        decision.id = dpm_model::DecisionId::new();
        decision.key = dpm_model::Key::new(format!("DEC-EXTRA-{index}"));
        plan.work_items.get_mut(&work).expect("work").condition = Some(dpm_model::WorkCondition {
            decision: decision.id,
            option: "A".into(),
        });
        plan.decisions.insert(decision.id, decision);
    }
    let summary = status(&plan, false, t(1)).expect("status");
    let choices = summary.open_choices.expect("open choices");
    assert_eq!(choices.scenario_count, 32);
    assert!(choices.scenario_count > dpm_engine::MAX_SCENARIOS);
    assert!(choices.scenarios.is_empty());
}
