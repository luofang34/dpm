use dpm_engine::{Command, Transition, apply_command, explain_work, gate_report};
use dpm_model::{ActorId, Plan, StartBasis};

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
        apply_command(&mut plan, actor, command, at(hour)).expect("execute");
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
