use super::*;

#[test]
fn blocking_and_rejection_keep_the_start_and_drop_the_rejected_submission() {
    let (mut plan, a, _) = pair(DependencyKind::FinishStart, 0.0, DependencyPolicy::Hard);
    let reviewer = ActorId::human("reviewer");
    ok(&mut plan, &worker(), Command::Claim { work: a }, 3);
    ok(&mut plan, &worker(), Command::Start { work: a }, 5);
    let block = Command::Block {
        work: a,
        reason: "vendor".into(),
    };
    ok(&mut plan, &worker(), block, 6);
    let report = Command::ReportProgress {
        work: a,
        percent: 40,
        note: None,
    };
    ok(&mut plan, &worker(), report, 6);
    ok(&mut plan, &worker(), Command::Unblock { work: a }, 7);
    let resumed = &plan.work_items[&a];
    assert_eq!(
        resumed.execution.status,
        WorkStatus::InProgress,
        "started work resumes started"
    );
    assert_eq!(resumed.execution.events.started_at, Some(t(5)));
    ok(&mut plan, &worker(), submit(a), 8);
    let reject = Command::Reject {
        work: a,
        reason: "missing test".into(),
    };
    ok(&mut plan, &reviewer, reject, 9);
    assert_eq!(plan.work_items[&a].execution.events.submitted_at, None);
    ok(&mut plan, &worker(), submit(a), 10);
    assert_eq!(
        plan.work_items[&a].execution.events.submitted_at,
        Some(t(10))
    );
}
