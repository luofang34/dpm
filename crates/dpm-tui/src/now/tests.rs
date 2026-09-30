use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{Command, NextWorkQuery, apply_command, next_work, status};
use dpm_model::{ActorId, DecisionId, DecisionStatus, Key, Plan, WorkItemId};

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

fn id(plan: &Plan, key: &str) -> WorkItemId {
    plan.find_work_by_key(key).expect("work").id
}

fn run(plan: &mut Plan, actor: &str, command: Command, hour: i64) {
    let label = format!("{command:?}");
    apply_command(
        plan,
        ActorId::human(actor),
        command,
        t(hour),
        dpm_model::OperationId::new(),
    )
    .unwrap_or_else(|e| panic!("{label}: {e}"));
}

/// Supplier A is chosen and its quote started, then blocked. The lead switches to supplier B, so
/// the blocked A quote is kept in flight outside the counted scope, while the B quote is blocked
/// inside it.
fn switched_with_blocked_work() -> Plan {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/conditional-plan.json"
    ))
    .expect("fixture");
    let supplier = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let decide = Command::Decide {
        decision: supplier,
        outcome: "A".into(),
    };
    run(&mut plan, "lead", decide, 0);
    let design = id(&plan, "SUP-DESIGN");
    run(&mut plan, "worker", Command::Claim { work: design }, 0);
    run(
        &mut plan,
        "worker",
        Command::Start {
            work: design,
            occurred_at: None,
        },
        0,
    );
    let submit = Command::Submit {
        work: design,
        note: None,
        occurred_at: None,
    };
    run(&mut plan, "worker", submit, 0);
    let verify = Command::Verify {
        work: design,
        note: None,
        occurred_at: None,
    };
    run(&mut plan, "lead", verify, 1);
    let quote = id(&plan, "SUP-A-QUOTE");
    run(&mut plan, "worker", Command::Claim { work: quote }, 2);
    run(
        &mut plan,
        "worker",
        Command::Start {
            work: quote,
            occurred_at: None,
        },
        2,
    );
    let block = Command::Block {
        work: quote,
        reason: "supplier A stopped answering".into(),
    };
    run(&mut plan, "worker", block, 3);
    let change =
        dpm_engine::plan_change(&plan, &switch_to_b(&plan, supplier), "Supplier A withdrew")
            .expect("delta");
    run(&mut plan, "lead", change, 4);
    let b_quote = id(&plan, "SUP-B-QUOTE");
    run(&mut plan, "other", Command::Claim { work: b_quote }, 5);
    run(
        &mut plan,
        "other",
        Command::Start {
            work: b_quote,
            occurred_at: None,
        },
        5,
    );
    let block = Command::Block {
        work: b_quote,
        reason: "awaiting B's price list".into(),
    };
    run(&mut plan, "other", block, 5);
    plan
}

fn switch_to_b(plan: &Plan, supplier: DecisionId) -> Plan {
    let mut proposed = plan.clone();
    let old = proposed.decisions.get_mut(&supplier).expect("decision");
    old.status = DecisionStatus::Superseded;
    let mut replacement = old.clone();
    replacement.id = DecisionId::new();
    replacement.key = Key::new("DEC-SUPPLIER-B");
    replacement.status = DecisionStatus::Decided;
    replacement.outcome = Some("B".into());
    replacement.resolved_at = None;
    replacement.rationale = Some("Supplier A withdrew its quote".into());
    replacement.supersedes = Some(old.id);
    proposed.decisions.insert(replacement.id, replacement);
    proposed
}

/// Lines under `title` up to the next blank line.
fn section<'a>(text: &'a str, title: &str) -> Vec<&'a str> {
    text.split("\n\n")
        .find_map(|block| block.strip_prefix(&format!("{title}:\n")))
        .map(|body| body.lines().collect())
        .unwrap_or_default()
}

#[test]
fn lists_follow_the_status_scope_and_excluded_in_flight_work_is_shown_apart() {
    let plan = switched_with_blocked_work();
    let summary = status(&plan, false, t(6)).expect("status");
    assert_eq!((summary.blocked, summary.excluded_in_flight), (1, 1));
    let candidates = next_work(&plan, &NextWorkQuery::default(), t(6)).expect("next");
    let timeline = dpm_model::Timeline::at(&plan, t(6));
    let text = super::text(&plan, &summary, &timeline, candidates);
    let blocked = section(&text, "Blocked work");
    assert_eq!(blocked.len(), 1, "{text}");
    assert!(blocked[0].starts_with("SUP-B-QUOTE"), "{text}");
    let excluded = section(&text, "Excluded in flight");
    assert_eq!(excluded.len(), 1, "{text}");
    assert!(
        excluded[0].starts_with("SUP-A-QUOTE") && excluded[0].contains("Blocked"),
        "{text}"
    );
    assert!(excluded[0].contains("DEC-SUPPLIER-B"), "{text}");
    assert!(text.contains("Excluded in flight 1"), "{text}");
}
