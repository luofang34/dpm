use super::*;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{Command, Operation, apply_command};
use dpm_model::{ActorId, OperationId, Plan};

fn at(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("instant")
        + TimeDelta::hours(hours)
}

fn start(work: dpm_model::WorkItemId) -> Command {
    Command::Start {
        work,
        occurred_at: None,
    }
}

fn submit(work: dpm_model::WorkItemId) -> Command {
    Command::Submit {
        work,
        note: None,
        occurred_at: None,
    }
}

fn verify(work: dpm_model::WorkItemId) -> Command {
    Command::Verify {
        work,
        note: None,
        occurred_at: None,
    }
}

/// Agent TEST-A (2 h of 4 h), TEST-D claimed, started and submitted in one sitting, and TEST-B
/// started and still in progress.
fn report() -> CalibrationReport {
    let mut plan: Plan =
        serde_json::from_str(include_str!("../../tests/support/execution-plan.json"))
            .expect("fixture");
    let mut history: Vec<Operation> = Vec::new();
    let [a, b, d] =
        ["TEST-A", "TEST-B", "TEST-D"].map(|key| plan.find_work_by_key(key).expect("work").id);
    let gate = plan.find_decision_by_key("TEST-GATE").expect("gate").id;
    let decide = Command::Decide {
        decision: gate,
        outcome: "Proceed".into(),
    };
    let steps = [
        ("agent:coder", Command::Claim { work: a }, 0),
        ("agent:coder", start(a), 1),
        ("agent:coder", submit(a), 3),
        ("human:bob", verify(a), 4),
        ("human:lead", decide, 4),
        ("agent:coder", Command::Claim { work: d }, 5),
        ("agent:coder", start(d), 5),
        ("agent:coder", submit(d), 5),
        ("human:bob", verify(d), 6),
        ("agent:coder", Command::Claim { work: b }, 7),
        ("agent:coder", start(b), 8),
    ];
    for (who, command, hours) in steps {
        let actor: ActorId = who.parse().expect("actor");
        let operation = apply_command(&mut plan, actor, command, at(hours), OperationId::new())
            .expect("command");
        history.push(operation);
    }
    dpm_engine::calibration(&plan, &history, at(10)).expect("calibration")
}

#[test]
fn the_summary_speaks_in_plain_words_and_names_what_it_left_out() {
    let text = summary(&report());
    for expected in [
        "actual time in elapsed hours",
        "  agent: median 0.500 (IQR 0.500-0.500) over 1, too few to apply; excluded bulk_recorded 1",
        "  by capability: agent/rust 0.500 over 1, agent/testing 0.500 over 1\n",
        "  excluded bulk_recorded: 1 (TEST-D)",
        "  human: median",
        "Flow: cycle median 3.0h over 1 (1 excluded)",
        "  agent: 3 episode(s): 2 verified (0 after rejection), 0 released, 0 handed off, 1 open; verified fraction 100%",
        "  aging TEST-B in_progress held 3.0h",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in\n{text}");
    }
    for debug in ["Human", "Agent", "InProgress", "BulkRecorded", "Elapsed"] {
        assert!(!text.contains(debug), "{debug:?} in\n{text}");
    }
}

#[test]
fn long_exclusion_lists_are_counted_not_listed() {
    let exclusion = Exclusion {
        reason: dpm_engine::ExclusionReason::NotInHistory,
        count: LISTED_KEYS + 1,
        keys: (0..=LISTED_KEYS)
            .map(|n| dpm_model::Key::new(format!("K-{n}")))
            .collect(),
    };
    assert_eq!(excluded(&exclusion), "  excluded not_in_history: 6");
}

#[test]
fn capability_groups_holding_only_exclusions_stay_out_of_the_summary() {
    let mut report = report();
    let mut unmeasured = report
        .estimates
        .by_capability
        .first()
        .cloned()
        .expect("group");
    unmeasured.capability = Some("design".into());
    unmeasured.samples = 0;
    unmeasured.median = None;
    report.estimates.by_capability.push(unmeasured);
    let text = summary(&report);
    assert!(!text.contains("agent/design"), "{text}");
    assert!(text.contains("agent/rust 0.500 over 1"), "{text}");
}
