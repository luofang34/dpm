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
    let report = gate_report(&plan, work).expect("gates");
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
    assert_eq!(explain_work(&plan, work).expect("explain").gates, report);
    assert_eq!(status(&plan, false).expect("status").gates[&work], report);
    assert_eq!(is_ready(&plan, &plan.work_items[&work]), report.ready);
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
fn temporal_overlap_never_satisfies_the_execution_dependency_gate() {
    let base: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let predecessor = base.find_work_by_key("TEST-A").expect("task").id;
    let work = base.find_work_by_key("TEST-B").expect("task").id;
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
        let report = gate_report(&plan, work).expect("gates");
        assert!(!report.ready, "{kind:?}");
        assert!(
            report.unmet.iter().any(|g| matches!(
                g,
                UnmetGate::Dependency { predecessor: p, relation, .. }
                    if *p == predecessor && *relation == kind
            )),
            "{kind:?}"
        );
        assert!(!is_ready(&plan, &plan.work_items[&work]), "{kind:?}");
    }
}
