#![allow(clippy::expect_used)]
use super::*;
use crate::{Command, apply_command, is_ready};
use chrono::Utc;
use dpm_model::{ActorId, Dependency, DependencyKind, Key};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}
fn apply(plan: &mut Plan, command: Command) {
    apply_command(
        plan,
        ActorId::agent("owner"),
        command,
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("command");
}
#[test]
fn reports_and_submission_do_not_bypass_verification_or_milestone_conditions() {
    let mut plan = fixture();
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    let milestone = plan.find_work_by_key("TEST-M1").expect("milestone").id;
    plan.dependencies.retain(|d| d.successor != milestone);
    plan.dependencies.push(Dependency::new(
        work,
        milestone,
        DependencyKind::FinishStart,
        0.0,
    ));
    apply(&mut plan, Command::Claim { work });
    apply(&mut plan, Command::Start { work });
    // One clock reading for both forecasts: started work's remaining duration depends on it, and
    // only the progress report may differ between them.
    let now = chrono::Utc::now();
    let schedule = dpm_schedule::deterministic_remaining(&plan, now).expect("schedule");
    apply(
        &mut plan,
        Command::ReportProgress {
            work,
            percent: 100,
            note: None,
        },
    );
    assert_eq!(
        plan.work_items[&work].execution.status,
        WorkStatus::InProgress
    );
    assert_eq!(
        dpm_schedule::deterministic_remaining(&plan, now)
            .expect("schedule")
            .project_finish_hours,
        schedule.project_finish_hours
    );
    assert_eq!(
        progress(&plan, chrono::Utc::now()).expect("progress").work[&work],
        ProgressSummary {
            percent_complete: 100.0,
            verified: false,
            completed_at: None,
            scope: crate::ProgressScope::Counted,
        }
    );
    assert_eq!(
        progress(&plan, chrono::Utc::now()).expect("progress").work[&milestone].percent_complete,
        0.0
    );
    let after = plan.find_work_by_key("TEST-B").expect("after");
    assert!(!is_ready(&plan, after, chrono::Utc::now()));
    apply(&mut plan, Command::Submit { work, note: None });
    assert!(!progress(&plan, chrono::Utc::now()).expect("progress").work[&work].verified);
    apply_command(
        &mut plan,
        ActorId::human("reviewer"),
        Command::Verify { work, note: None },
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("verify");
    let reached = progress(&plan, chrono::Utc::now()).expect("progress").work[&milestone];
    assert_eq!((reached.percent_complete, reached.verified), (100.0, true));
    assert_eq!(
        reached.completed_at,
        plan.work_items[&work]
            .execution
            .events
            .verified_at
            .map(dpm_model::EventTime::Recorded)
    );
    assert_eq!(
        plan.work_items[&milestone].execution.status,
        WorkStatus::Planned
    );
    assert_eq!(plan.work_items[&milestone].expected_duration_hours(), 0.0);
}
#[test]
fn report_validation_is_atomic_and_blocked_corrections_preserve_the_blocker() {
    let mut plan = fixture();
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    let report = Command::ReportProgress {
        work,
        percent: 50,
        note: None,
    };
    let before = plan.clone();
    assert!(
        apply_command(
            &mut plan,
            ActorId::agent("owner"),
            report.clone(),
            Utc::now(),
            dpm_model::OperationId::new()
        )
        .is_err()
    );
    assert_eq!(plan, before);
    apply(&mut plan, Command::Claim { work });
    apply(&mut plan, Command::Start { work });
    for (actor, command) in [
        ("other", report.clone()),
        (
            "owner",
            Command::ReportProgress {
                work,
                percent: 101,
                note: None,
            },
        ),
    ] {
        let before = plan.clone();
        assert!(
            apply_command(
                &mut plan,
                ActorId::agent(actor),
                command,
                Utc::now(),
                dpm_model::OperationId::new()
            )
            .is_err()
        );
        assert_eq!(plan, before);
    }
    apply(&mut plan, report);
    apply(
        &mut plan,
        Command::Block {
            work,
            reason: "waiting".into(),
        },
    );
    apply(
        &mut plan,
        Command::ReportProgress {
            work,
            percent: 25,
            note: Some("rework discovered".into()),
        },
    );
    assert_eq!(plan.work_items[&work].execution.status, WorkStatus::Blocked);
    assert_eq!(
        plan.work_items[&work].execution.block_reason.as_deref(),
        Some("waiting")
    );
    assert_eq!(
        progress(&plan, chrono::Utc::now()).expect("progress").work[&work].percent_complete,
        25.0
    );
}
#[test]
fn nested_packages_average_leaf_tasks_once_and_legacy_submissions_show_100_percent() {
    let mut plan = fixture();
    plan.dependencies.clear();
    plan.decisions.clear();
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    let mut package = plan.work_items[&a].clone();
    package.id = WorkItemId::new();
    package.key = Key::new("WP");
    package.kind = WorkKind::WorkPackage;
    package.schedule.estimate = None;
    let parent = package.id;
    plan.work_items.insert(parent, package.clone());
    package.id = WorkItemId::new();
    package.key = Key::new("NESTED");
    package.parent = Some(parent);
    let nested = package.id;
    plan.work_items.insert(nested, package);
    plan.work_items.get_mut(&a).expect("a").parent = Some(parent);
    plan.work_items.get_mut(&b).expect("b").parent = Some(nested);
    apply(&mut plan, Command::Claim { work: a });
    apply(&mut plan, Command::Start { work: a });
    apply(
        &mut plan,
        Command::ReportProgress {
            work: a,
            percent: 50,
            note: None,
        },
    );
    apply(&mut plan, Command::Claim { work: b });
    apply(&mut plan, Command::Start { work: b });
    apply(
        &mut plan,
        Command::Submit {
            work: b,
            note: None,
        },
    );
    assert_eq!(plan.work_items[&b].execution.reported_progress_percent, 0);
    let before = plan.clone();
    let projection = progress(&plan, chrono::Utc::now()).expect("progress");
    assert_eq!(projection.work[&parent].percent_complete, 75.0);
    assert_eq!(projection.work[&nested].percent_complete, 100.0);
    assert!(!projection.work[&nested].verified);
    let tasks = plan
        .work_items
        .values()
        .filter(|w| w.is_executable())
        .count();
    assert!((projection.overall.percent_complete - 150.0 / tasks as f64).abs() < 1e-9);
    assert_eq!(plan, before);
}
#[test]
fn import_defaults_reports_to_zero_and_rejects_invalid_progress() {
    let mut plan = fixture();
    assert!(
        plan.work_items
            .values()
            .all(|w| w.execution.reported_progress_percent == 0)
    );
    let task = plan.find_work_by_key("TEST-A").expect("task").id;
    plan.work_items
        .get_mut(&task)
        .expect("task")
        .execution
        .reported_progress_percent = 50;
    assert!(plan.validate().is_err());
    plan.work_items
        .get_mut(&task)
        .expect("task")
        .execution
        .reported_progress_percent = 101;
    assert!(plan.validate().is_err());
    let empty = progress(&Plan::empty("empty"), chrono::Utc::now()).expect("empty");
    assert_eq!(
        empty.overall,
        ProgressSummary {
            percent_complete: 0.0,
            verified: false,
            completed_at: None,
            scope: crate::ProgressScope::Counted,
        }
    );
}
