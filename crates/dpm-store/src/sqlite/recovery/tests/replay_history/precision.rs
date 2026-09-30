//! Replay compares plans decoded from stored JSON text, so every float must survive a text round
//! trip bit for bit; otherwise an honest history diverges from its own snapshot.

use super::*;

/// A lag whose shortest decimal form is misread by a parser that is not correctly rounded.
const HARD_LAG: f64 = 1.929_842_284_342_272e-18;

fn with_lag(lag: &str) -> Plan {
    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    value["dependencies"][0]["lag_hours"] = serde_json::from_str(lag).expect("number");
    serde_json::from_value(value).expect("plan")
}

#[test]
fn floats_survive_the_text_round_trip_exactly() {
    let text = serde_json::to_string(&HARD_LAG).expect("json");
    let parsed: f64 = serde_json::from_str(&text).expect("parse");
    assert_eq!(parsed.to_bits(), HARD_LAG.to_bits(), "{text}");
    let plan = with_lag("1.929842284342272e-18");
    assert_eq!(plan.dependencies[0].lag_hours.to_bits(), HARD_LAG.to_bits());
}

#[test]
fn an_imported_hard_float_replays_after_lifecycle_commands() {
    let dir = tempfile::tempdir().expect("directory");
    let mut r = Recorder::with_plan(dir.path(), &with_lag("1.929842284342272e-18"));
    let a = r.id("TEST-A");
    r.run(lead(), Command::Claim { work: a });
    r.run(
        lead(),
        Command::Start {
            work: a,
            occurred_at: None,
        },
    );
    let lag = r.plan().dependencies[0].lag_hours;
    assert_eq!(lag.to_bits(), HARD_LAG.to_bits());
}

#[test]
fn a_hard_float_introduced_by_a_plan_change_replays_and_stays_fresh() {
    let dir = tempfile::tempdir().expect("directory");
    let mut r = Recorder::new(dir.path());
    let last = r.plan().dependencies.len() - 1;
    r.change(|plan| plan.dependencies[last].lag_hours = HARD_LAG);
    // The next change's `before` is read back from the stored snapshot and log text.
    r.change(|plan| plan.dependencies[last].lag_hours = HARD_LAG * 3.0);
    let a = r.id("TEST-A");
    r.run(lead(), Command::Claim { work: a });
    let lag = r.plan().dependencies[last].lag_hours;
    assert_eq!(lag.to_bits(), (HARD_LAG * 3.0).to_bits());
}
