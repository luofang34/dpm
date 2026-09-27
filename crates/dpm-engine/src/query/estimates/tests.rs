use super::*;
use crate::{Command, NextWorkQuery, apply_command, explain_work, next_work, status};
use chrono::{DateTime, TimeZone, Utc};
use dpm_model::{ActorId, WorkStatus};

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
}

fn execution() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn conditional() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/conditional-plan.json"
    ))
    .expect("fixture")
}

fn drop_estimate(plan: &mut Plan, key: &str) {
    let id = plan.find_work_by_key(key).expect("work").id;
    if let Some(work) = plan.work_items.get_mut(&id) {
        work.schedule.estimate = None;
    }
}

fn set_status(plan: &mut Plan, key: &str, status: WorkStatus) {
    let id = plan.find_work_by_key(key).expect("work").id;
    if let Some(work) = plan.work_items.get_mut(&id) {
        work.execution.status = status;
    }
}

fn listed(plan: &Plan) -> Vec<String> {
    unestimated(plan, &Timeline::at(plan, at()))
        .into_iter()
        .map(|k| k.0)
        .collect()
}

#[test]
fn only_outstanding_tasks_without_an_estimate_are_listed_in_key_order() {
    let mut plan = execution();
    let milestone = plan.find_work_by_key("TEST-M1").expect("milestone");
    assert!(
        milestone.schedule.estimate.is_none(),
        "milestones carry no estimate"
    );
    assert!(listed(&plan).is_empty(), "estimated tasks and milestones");

    for key in ["TEST-E", "TEST-C", "TEST-A"] {
        drop_estimate(&mut plan, key);
    }
    assert_eq!(listed(&plan), ["TEST-A", "TEST-C", "TEST-E"]);

    // Submitted work keeps its full duration until verification; unestimated, that duration is 0 h.
    set_status(&mut plan, "TEST-A", WorkStatus::Submitted);
    set_status(&mut plan, "TEST-C", WorkStatus::Verified);
    set_status(&mut plan, "TEST-E", WorkStatus::Done);
    assert_eq!(listed(&plan), ["TEST-A"]);
}

#[test]
fn status_explain_and_next_report_the_same_unestimated_work() {
    let mut plan = execution();
    drop_estimate(&mut plan, "TEST-A");
    let summary = status(&plan, false, at()).expect("status");
    assert_eq!(summary.unestimated, [Key("TEST-A".into())]);

    let a = plan.find_work_by_key("TEST-A").expect("work").id;
    let b = plan.find_work_by_key("TEST-B").expect("work").id;
    let explained = explain_work(&plan, a, at()).expect("explain");
    assert!(explained.unestimated);
    assert!(explained.why_now.iter().any(|r| r.contains("0 h")));
    assert!(!explain_work(&plan, b, at()).expect("explain").unestimated);

    let query = NextWorkQuery {
        use_probabilistic_criticality: false,
        ..NextWorkQuery::default()
    };
    let candidates = next_work(&plan, &query, at()).expect("next");
    let first = candidates.iter().find(|c| c.work.id == a).expect("ready");
    assert!(first.reasons.iter().any(|r| r.contains("0 h")));
}

#[test]
fn work_outside_the_active_graph_is_listed_only_by_the_scenario_that_selects_it() {
    let mut plan = conditional();
    drop_estimate(&mut plan, "SUP-A-QUOTE");
    drop_estimate(&mut plan, "SUP-B-QUOTE");

    let open = status(&plan, false, at()).expect("status");
    assert!(
        open.unestimated.is_empty(),
        "undecided branches are not in the headline"
    );
    let choices = open.open_choices.expect("open choices");
    let by_option: Vec<(String, Vec<Key>)> = choices
        .scenarios
        .iter()
        .map(|s| (s.choices.values().cloned().collect(), s.unestimated.clone()))
        .collect();
    assert_eq!(
        by_option,
        [
            ("A".into(), vec![Key("SUP-A-QUOTE".into())]),
            ("B".into(), vec![Key("SUP-B-QUOTE".into())]),
        ]
    );

    let decision = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let decide = Command::Decide {
        decision,
        outcome: "B".into(),
    };
    apply_command(
        &mut plan,
        ActorId::human("lead"),
        decide,
        at(),
        dpm_model::OperationId::new(),
    )
    .expect("decide");
    assert_eq!(
        listed(&plan),
        ["SUP-B-QUOTE"],
        "not-selected branch excluded"
    );
}

/// The list must name exactly the tasks the remaining projection counts at 0 h for lack of an
/// estimate, so a change to how the projection treats lifecycle or applicability fails here.
#[test]
fn the_list_matches_the_tasks_the_remaining_projection_runs_at_zero_hours() {
    let mut plan = conditional();
    for key in ["SUP-DESIGN", "SUP-A-QUOTE", "SUP-B-QUOTE", "SUP-BUILD"] {
        drop_estimate(&mut plan, key);
    }
    let decision = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let decide = Command::Decide {
        decision,
        outcome: "B".into(),
    };
    apply_command(
        &mut plan,
        ActorId::human("lead"),
        decide,
        at(),
        dpm_model::OperationId::new(),
    )
    .expect("decide");
    let design = plan.find_work_by_key("SUP-DESIGN").expect("work").id;
    let quote = plan.find_work_by_key("SUP-B-QUOTE").expect("work").id;
    for (actor, command) in [
        ("worker", Command::Claim { work: design }),
        ("worker", Command::Start { work: design }),
        (
            "worker",
            Command::Submit {
                work: design,
                note: None,
            },
        ),
        (
            "lead",
            Command::Verify {
                work: design,
                note: None,
            },
        ),
        ("worker", Command::Claim { work: quote }),
        ("worker", Command::Start { work: quote }),
        (
            "worker",
            Command::Submit {
                work: quote,
                note: None,
            },
        ),
    ] {
        let actor = if actor == "lead" {
            ActorId::human(actor)
        } else {
            ActorId::agent(actor)
        };
        apply_command(
            &mut plan,
            actor,
            command,
            at(),
            dpm_model::OperationId::new(),
        )
        .expect("execute");
    }

    let schedule = dpm_schedule::deterministic_remaining(&plan, at()).expect("schedule");
    let projected: Vec<String> = plan
        .work_items
        .values()
        .filter(|w| w.is_executable() && w.schedule.estimate.is_none())
        .filter(|w| !w.execution.status.satisfies_dependency())
        .filter(|w| schedule.activities.contains_key(&w.id))
        .map(|w| w.key.0.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(listed(&plan), projected);
    assert_eq!(listed(&plan), ["SUP-B-QUOTE", "SUP-BUILD"]);
}
