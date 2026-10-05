//! A reviewed change that comes back in another number encoding is the same change.

use super::{first_key, fixtures};
use crate::change::{EntityChange, differences, patch};
use crate::{Command, EngineError, apply_command};
use dpm_model::{ActorId, OperationId};

/// Rewrites every whole floating-point number as an integer, as a client's JSON encoder may.
fn integral(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e15 => {
                serde_json::json!(f as i64)
            }
            _ => value.clone(),
        },
        serde_json::Value::Array(items) => items.iter().map(integral).collect(),
        serde_json::Value::Object(map) => {
            map.iter().map(|(k, v)| (k.clone(), integral(v))).collect()
        }
        other => other.clone(),
    }
}

#[test]
fn a_reviewed_change_re_encoded_with_whole_numbers_still_applies_and_a_real_difference_does_not() {
    let (_, current) = fixtures().remove(0);
    let mut proposed = current.clone();
    let id = first_key(&proposed.work_items).expect("work");
    if let Some(work) = proposed.work_items.get_mut(&id) {
        work.title = "Renamed by a client".into();
    }
    let changes = differences(&current, &proposed).expect("diff");
    let reencoded: Vec<_> = changes
        .iter()
        .map(|c| EntityChange {
            before: integral(&c.before),
            after: integral(&c.after),
            ..c.clone()
        })
        .collect();
    assert_eq!(patch(&current, &reencoded).expect("patch"), proposed);

    let mut drifted = reencoded;
    if let Some(change) = drifted.first_mut() {
        change.before["title"] = serde_json::json!("Not the current title");
    }
    assert!(matches!(
        patch(&current, &drifted),
        Err(EngineError::StaleChange { .. })
    ));
}

#[test]
fn a_re_encoded_reviewed_change_is_recorded_in_its_canonical_form() {
    let (_, mut current) = fixtures().remove(0);
    let mut proposed = current.clone();
    let id = first_key(&proposed.work_items).expect("work");
    if let Some(work) = proposed.work_items.get_mut(&id) {
        work.title = "Renamed by a client".into();
    }
    let canonical = differences(&current, &proposed).expect("diff");
    let reencoded: Vec<_> = canonical
        .iter()
        .map(|c| EntityChange {
            before: integral(&c.before),
            after: integral(&c.after),
            ..c.clone()
        })
        .collect();
    let command = Command::ApplyChange {
        changes: reencoded,
        reason: "Rename".into(),
    };
    let operation = apply_command(
        &mut current,
        ActorId::human("lead"),
        command,
        chrono::Utc::now(),
        OperationId::new(),
    )
    .expect("a re-encoded reviewed change applies");
    match operation.command {
        Command::ApplyChange { changes, .. } => assert_eq!(changes, canonical),
        other => panic!("unexpected command {other:?}"),
    }
}
