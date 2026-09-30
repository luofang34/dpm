//! Public execution contract across an agent and an independent reviewer.
#![cfg(test)]

use chrono::Utc;
use dpm_engine::{Command, NextWorkQuery, apply_command, explain_work, is_ready, next_work};
use dpm_model::{ActorId, Plan, WorkStatus};

fn fixture() -> Plan {
    serde_json::from_str(include_str!("../../../tests/support/execution-plan.json"))
        .expect("synthetic execution graph must deserialize")
}

fn id(plan: &Plan, key: &str) -> dpm_model::WorkItemId {
    plan.find_work_by_key(key).expect("fixture key").id
}

#[test]
fn agent_and_human_execution_loop_unlocks_work_semantically() {
    let mut plan = fixture();
    let query = NextWorkQuery {
        use_probabilistic_criticality: false,
        ..NextWorkQuery::default()
    };

    let first = next_work(&plan, &query, chrono::Utc::now()).expect("next work");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].work.key.0, "TEST-A");

    let input = id(&plan, "TEST-A");
    complete_input(&mut plan, input);

    // The predecessor is complete, but the explicit human decision gate still prevents branch
    // and parallel work from silently becoming executable.
    assert!(
        next_work(&plan, &query, chrono::Utc::now())
            .expect("next work")
            .is_empty()
    );
    let branch = id(&plan, "TEST-B");
    assert!(!is_ready(
        &plan,
        &plan.work_items[&branch],
        chrono::Utc::now()
    ));
    let explanation = explain_work(&plan, branch, chrono::Utc::now()).expect("explain");
    assert!(
        explanation
            .why_now
            .iter()
            .any(|line| line.contains("TEST-GATE"))
    );

    let decision = plan.find_decision_by_key("TEST-GATE").expect("decision").id;
    apply_command(
        &mut plan,
        ActorId::human("reviewer"),
        Command::Decide {
            decision,
            outcome: "Accept the verified input".into(),
        },
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("decide");

    let keys = next_work(&plan, &query, chrono::Utc::now())
        .expect("next work")
        .into_iter()
        .map(|candidate| candidate.work.key.0)
        .collect::<Vec<_>>();
    assert!(keys.contains(&"TEST-B".to_string()));
    assert!(keys.contains(&"TEST-D".to_string()));

    // A blocker changes executable work immediately without rewriting the baseline schedule.
    apply_command(
        &mut plan,
        ActorId::human("operator"),
        Command::Block {
            work: branch,
            reason: "Additional review pending".into(),
        },
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("block");
    assert_eq!(
        plan.work_items[&branch].execution.status,
        WorkStatus::Blocked
    );
    let keys = next_work(&plan, &query, chrono::Utc::now())
        .expect("next work")
        .into_iter()
        .map(|candidate| candidate.work.key.0)
        .collect::<Vec<_>>();
    assert!(!keys.contains(&"TEST-B".to_string()));
    assert!(keys.contains(&"TEST-D".to_string()));
}

fn complete_input(plan: &mut Plan, input: dpm_model::WorkItemId) {
    let agent = ActorId::agent("test-worker");
    apply_command(
        plan,
        agent.clone(),
        Command::Claim { work: input },
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("claim");
    apply_command(
        plan,
        agent.clone(),
        Command::Start {
            work: input,
            occurred_at: None,
        },
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("start");
    apply_command(
        plan,
        agent,
        Command::Submit {
            work: input,
            note: Some("input documented".into()),
            occurred_at: None,
        },
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("submit");
    apply_command(
        plan,
        ActorId::human("reviewer"),
        Command::Verify {
            work: input,
            note: Some("criteria checked".into()),
            occurred_at: None,
        },
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("verify");
}

#[test]
fn equally_ranked_candidates_follow_natural_key_order() {
    let mut plan = fixture();
    let template = plan.find_work_by_key("TEST-A").expect("ready task").clone();
    for key in ["N-10", "N-2"] {
        let work = dpm_model::WorkItem {
            id: dpm_model::WorkItemId::new(),
            key: dpm_model::Key::new(key),
            ..template.clone()
        };
        plan.work_items.insert(work.id, work);
    }
    let query = NextWorkQuery {
        use_probabilistic_criticality: false,
        ..NextWorkQuery::default()
    };
    let ranked: Vec<_> = next_work(&plan, &query, Utc::now())
        .expect("next work")
        .into_iter()
        .map(|c| c.work.key.0)
        .filter(|k| k.starts_with("N-"))
        .collect();
    assert_eq!(ranked, ["N-2", "N-10"]);
}
