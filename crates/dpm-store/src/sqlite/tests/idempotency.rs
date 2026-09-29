//! Identity lookups, lineage preconditions and lock timeouts inside the write transaction.

use super::*;
use std::time::Duration;

#[test]
fn a_recorded_identity_is_found_and_answered_before_any_precondition() {
    let mut store = SqliteStore::in_memory_blocking().expect("store");
    let mut plan = fixture();
    store.initialize_blocking(&plan).expect("initialize");
    let op = claim(&mut plan, "owner");
    let committed = store.persist_blocking(&plan, &op, None).expect("persist");
    let found = store
        .recorded_operation_blocking(op.id)
        .expect("lookup")
        .expect("recorded");
    assert_eq!(
        serde_json::to_value(&found).expect("json"),
        serde_json::to_value(&committed).expect("json")
    );
    assert_eq!(found.operation.workspace_id, plan.workspace.id);
    let lineage = store
        .lineage_blocking()
        .expect("lineage")
        .expect("initialized");
    assert_eq!(found.operation.lineage_id, lineage.lineage_id);
    // A resend carries the stale base revision; the identity is answered first, not the conflict.
    assert!(matches!(
        store.persist_blocking(&plan, &op, Some(dpm_model::LineageId::new())),
        Err(StoreError::DuplicateOperation { .. })
    ));
    assert!(
        store
            .recorded_operation_blocking(dpm_model::OperationId::new())
            .expect("lookup")
            .is_none()
    );
    let before = store.plan_before_blocking(found.sequence).expect("replay");
    assert_eq!(before, fixture());
}

#[test]
fn recorded_operations_round_trip_through_their_flat_json() {
    let mut store = SqliteStore::in_memory_blocking().expect("store");
    let mut plan = fixture();
    store.initialize_blocking(&plan).expect("initialize");
    let op = claim(&mut plan, "owner");
    let committed = store.persist_blocking(&plan, &op, None).expect("persist");
    let json = serde_json::to_value(&committed.operation).expect("json");
    for field in [
        "id",
        "base_revision",
        "command",
        "workspace_id",
        "lineage_id",
    ] {
        assert!(json.get(field).is_some(), "{field} in {json}");
    }
    let decoded: RecordedOperation = serde_json::from_value(json.clone()).expect("decode");
    assert_eq!(serde_json::to_value(decoded).expect("json"), json);
}

#[test]
fn a_different_expected_lineage_refuses_the_write() {
    let mut store = SqliteStore::in_memory_blocking().expect("store");
    let mut plan = fixture();
    store.initialize_blocking(&plan).expect("initialize");
    let initial = plan.clone();
    let lineage = store
        .lineage_blocking()
        .expect("lineage")
        .expect("initialized");
    assert!(!lineage.archived);
    let op = claim(&mut plan, "owner");
    let other = dpm_model::LineageId::new();
    assert!(matches!(
        store.persist_blocking(&plan, &op, Some(other)),
        Err(StoreError::Lineage(crate::LineageError::Mismatch { expected, actual }))
            if expected == other && actual == lineage.lineage_id
    ));
    assert_eq!(store.load_blocking().expect("load"), Some(initial));
    store
        .persist_blocking(&plan, &op, Some(lineage.lineage_id))
        .expect("the observed lineage writes");
}

#[test]
fn a_lock_held_past_the_timeout_is_reported_busy_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("plan.sqlite");
    let mut store = SqliteStore::open_blocking(&path).expect("store");
    let mut plan = fixture();
    store.initialize_blocking(&plan).expect("initialize");
    store
        .set_busy_timeout_blocking(Duration::from_millis(50))
        .expect("timeout");
    let holder = Connection::open(&path).expect("raw connection");
    holder
        .execute_batch("BEGIN IMMEDIATE")
        .expect("hold the write lock");
    let op = claim(&mut plan, "owner");
    let refused = store
        .persist_blocking(&plan, &op, None)
        .expect_err("locked");
    assert!(refused.is_busy(), "{refused:?}");
    assert!(!refused.is_corruption());
    holder.execute_batch("ROLLBACK").expect("release");
    assert_eq!(store.operation_count_blocking().expect("count"), 0);
    store
        .persist_blocking(&plan, &op, None)
        .expect("retry succeeds");
}
