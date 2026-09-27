use dpm_engine::{Command, Transition, apply_command, explain_work, gate_report};
use dpm_model::{ActorId, Plan, StartBasis};

#[test]
fn detail_dependency_lines_state_a_provisional_start_basis() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    plan.dependencies
        .iter_mut()
        .find(|d| d.predecessor == a && d.successor == b)
        .expect("edge")
        .start_basis = StartBasis::Provisional;
    let detail = crate::detail::text(
        &plan,
        &explain_work(&plan, b, chrono::Utc::now()).expect("explain"),
    );
    let line = detail
        .lines()
        .find(|line| line.starts_with("TEST-A --FS") && line.contains("TEST-B"))
        .expect("dependency line");
    assert!(line.ends_with(" [provisional start]"), "{line}");
}

#[test]
fn detail_states_applicability_in_words_with_keys() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/conditional-plan.json"
    ))
    .expect("fixture");
    let decision = plan.decisions.keys().next().copied().expect("decision");
    apply_command(
        &mut plan,
        ActorId::human("lead"),
        Command::Decide {
            decision,
            outcome: "B".into(),
        },
        chrono::Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("decide");
    let audit = plan.find_work_by_key("SUP-A-AUDIT").expect("audit").id;
    let detail = crate::detail::text(
        &plan,
        &explain_work(&plan, audit, chrono::Utc::now()).expect("explain"),
    );
    let line = detail
        .lines()
        .find(|line| line.starts_with("Applicability:"))
        .expect("applicability line");
    assert!(
        line.starts_with("Applicability: stranded — prerequisite SUP-A-QUOTE was not selected"),
        "{line}"
    );
    assert!(
        !line.contains("DependencyId(") && !line.contains("Key(\""),
        "{line}"
    );
}

#[test]
fn detail_shows_the_rejected_basis_and_affected_successor_from_the_shared_report() {
    use chrono::TimeZone;
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    plan.decisions.clear();
    plan.dependencies
        .iter_mut()
        .find(|d| d.predecessor == a && d.successor == b)
        .expect("edge")
        .start_basis = StartBasis::Provisional;
    let at = |hour| {
        chrono::Utc
            .with_ymd_and_hms(2026, 9, 1, hour, 0, 0)
            .single()
            .expect("time")
    };
    let steps = [
        (ActorId::agent("author"), Command::Claim { work: a }),
        (ActorId::agent("author"), Command::Start { work: a }),
        (
            ActorId::agent("author"),
            Command::Submit {
                work: a,
                note: None,
            },
        ),
        (ActorId::agent("builder"), Command::Claim { work: b }),
        (ActorId::agent("builder"), Command::Start { work: b }),
        (
            ActorId::human("reviewer"),
            Command::Reject {
                work: a,
                reason: "fails acceptance".into(),
            },
        ),
    ];
    for (hour, (actor, command)) in (1..).zip(steps) {
        apply_command(
            &mut plan,
            actor,
            command,
            at(hour),
            dpm_model::OperationId::new(),
        )
        .expect("execute");
    }
    let now = at(8);
    let successor = crate::detail::text(&plan, &explain_work(&plan, b, now).expect("explain"));
    let report = gate_report(&plan, b, Transition::Submit, now).expect("report");
    for reason in report.reasons() {
        assert!(
            successor.contains(&format!("submit: {reason}")),
            "{successor}"
        );
    }
    assert!(
        successor.contains("relies on TEST-A attempt #1: invalidated, rejected by human:reviewer"),
        "{successor}"
    );
    let predecessor = crate::detail::text(&plan, &explain_work(&plan, a, now).expect("explain"));
    assert!(
        predecessor.contains("TEST-B relies on attempt #1: invalidated"),
        "{predecessor}"
    );
}

#[test]
fn detail_says_in_words_when_its_task_counts_as_zero_hours() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    plan.work_items.get_mut(&a).expect("a").schedule.estimate = None;
    let text = |work| {
        crate::detail::text(
            &plan,
            &explain_work(&plan, work, chrono::Utc::now()).expect("explain"),
        )
    };
    assert!(text(a).contains("Unestimated: this task counts as 0 h"));
    assert!(!text(b).contains("Unestimated"));
}
