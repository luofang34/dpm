use super::*;
use crate::{Command, apply_command, explain_work, is_ready, status};
use chrono::Utc;

#[test]
fn structured_gates_report_every_constraint_and_inherited_decision() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let work = plan.find_work_by_key("TEST-B").expect("task").id;
    let mut edge = plan
        .dependencies
        .iter()
        .find(|d| d.successor == work)
        .expect("edge")
        .clone();
    edge.kind = DependencyKind::StartStart;
    edge.id = dpm_model::DependencyId::new();
    plan.dependencies.push(edge);
    let report =
        gate_report(&plan, work, crate::Transition::Claim, chrono::Utc::now()).expect("gates");
    assert!(!report.ready);
    assert_eq!(
        report
            .unmet
            .iter()
            .filter(|g| matches!(g, UnmetGate::Dependency { .. }))
            .count(),
        2
    );
    assert!(
        report
            .unmet
            .iter()
            .any(|g| matches!(g, UnmetGate::Decision { key } if key == "TEST-GATE"))
    );
    assert_eq!(
        explain_work(&plan, work, chrono::Utc::now())
            .expect("explain")
            .gates,
        report
    );
    assert_eq!(
        status(&plan, false, chrono::Utc::now())
            .expect("status")
            .gates[&work],
        report
    );
    assert_eq!(
        is_ready(&plan, &plan.work_items[&work], chrono::Utc::now()),
        report.ready
    );
    let before = plan.clone();
    assert!(
        apply_command(
            &mut plan,
            ActorId::agent("worker"),
            Command::Claim { work },
            Utc::now()
        )
        .is_err()
    );
    assert_eq!(plan, before);
}

#[test]
fn a_lead_never_satisfies_the_gate_of_the_transition_its_relation_governs() {
    let base: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let predecessor = base.find_work_by_key("TEST-A").expect("task").id;
    let work = base.find_work_by_key("TEST-B").expect("task").id;
    let now = chrono::Utc::now();
    for kind in [
        DependencyKind::FinishStart,
        DependencyKind::StartStart,
        DependencyKind::FinishFinish,
        DependencyKind::StartFinish,
    ] {
        let mut plan = base.clone();
        let edge = plan
            .dependencies
            .iter_mut()
            .find(|d| d.predecessor == predecessor && d.successor == work)
            .expect("edge");
        edge.kind = kind;
        edge.lag_hours = -1000.0;
        let schedule = dpm_schedule::deterministic(&plan).expect("schedule");
        // The lead lets the projection start the successor before its predecessor finishes.
        assert!(
            schedule.activities[&work].earliest_start_hours
                < schedule.activities[&predecessor].earliest_finish_hours,
            "{kind:?}"
        );
        let governing = if kind.successor_endpoint() == Endpoint::Start {
            crate::Transition::Claim
        } else {
            crate::Transition::Submit
        };
        for transition in crate::Transition::ALL {
            let report = gate_report(&plan, work, transition, now).expect("gates");
            let gated = report.unmet.iter().any(|g| {
                matches!(
                    g,
                    UnmetGate::Dependency { predecessor: p, relation, release: Release::AwaitingEvent, .. }
                        if *p == predecessor && *relation == kind
                )
            });
            let expected = transition == governing
                || transition == crate::Transition::Verify
                || (transition == crate::Transition::Start
                    && governing == crate::Transition::Claim);
            assert_eq!(gated, expected, "{kind:?} {transition:?}");
            if gated {
                assert!(report.reasons().iter().any(|r| r.contains("schedule only")));
            }
        }
    }
}
