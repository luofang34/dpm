use super::*;
use chrono::Utc;
use dpm_engine::Command;
use dpm_model::{ActorId, WorkStatus};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn claim(plan: &mut Plan, actor: &str) -> Operation {
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    apply_command(
        plan,
        ActorId::agent(actor),
        Command::Claim { work },
        Utc::now(),
    )
    .expect("claim")
}

#[test]
fn operation_and_snapshot_commit_together_and_survive_reopen() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("plan.sqlite");
    let mut store = SqliteStore::open_blocking(&path).expect("store");
    let mut plan = fixture();
    store.initialize_blocking(&plan).expect("initialize");
    let op = claim(&mut plan, "owner");
    store.persist_blocking(&plan, &op).expect("persist");
    drop(store);
    let reopened = SqliteStore::open_existing_blocking(&path).expect("reopen");
    assert_eq!(reopened.load_blocking().expect("load"), Some(plan));
    assert_eq!(reopened.operation_count_blocking().expect("count"), 1);
}

#[test]
fn initialization_cannot_overwrite_a_workspace_or_history() {
    let mut store = SqliteStore::in_memory_blocking().expect("store");
    let mut plan = fixture();
    store.initialize_blocking(&plan).expect("initialize");
    let op = claim(&mut plan, "owner");
    store.persist_blocking(&plan, &op).expect("persist");
    assert!(matches!(
        store.initialize_blocking(&Plan::empty("replacement")),
        Err(StoreError::AlreadyInitialized(_))
    ));
    assert_eq!(store.load_blocking().expect("load"), Some(plan));
    assert_eq!(store.operation_count_blocking().expect("count"), 1);
}

#[test]
fn a_write_without_an_initial_snapshot_is_rejected() {
    let mut store = SqliteStore::in_memory_blocking().expect("store");
    let mut plan = fixture();
    let op = claim(&mut plan, "owner");
    assert!(matches!(
        store.persist_blocking(&plan, &op),
        Err(StoreError::NotInitialized(_))
    ));
    assert!(store.load_blocking().expect("load").is_none());
    assert_eq!(store.operation_count_blocking().expect("count"), 0);
}

#[test]
fn stale_writers_cannot_overwrite_committed_operations() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("plan.sqlite");
    let mut first = SqliteStore::open_blocking(&path).expect("first");
    first.initialize_blocking(&fixture()).expect("initialize");
    let mut second = SqliteStore::open_existing_blocking(&path).expect("second");
    let mut a = first.load_blocking().expect("load").expect("plan");
    let mut b = second.load_blocking().expect("load").expect("plan");
    let op_a = claim(&mut a, "first");
    let op_b = claim(&mut b, "second");
    first.persist_blocking(&a, &op_a).expect("persist");
    assert!(matches!(
        second.persist_blocking(&b, &op_b),
        Err(StoreError::RevisionConflict { .. })
    ));
    assert_eq!(second.load_blocking().expect("load"), Some(a));
    assert_eq!(second.operation_count_blocking().expect("count"), 1);
}

#[test]
fn revision_wrap_survives_sqlite_and_rejects_skipped_revisions() {
    let mut store = SqliteStore::in_memory_blocking().expect("store");
    let mut plan = fixture();
    plan.revision = u64::MAX;
    store.initialize_blocking(&plan).expect("initialize");
    assert_eq!(
        store.load_blocking().expect("load").expect("plan").revision,
        u64::MAX
    );
    let mut op = claim(&mut plan, "owner");
    op.resulting_revision = 2;
    plan.revision = 2;
    assert!(matches!(
        store.persist_blocking(&plan, &op),
        Err(StoreError::InvalidOperationRevision { .. })
    ));
    op.resulting_revision = 0;
    plan.revision = 0;
    store.persist_blocking(&plan, &op).expect("wrapped write");
    assert_eq!(
        store.load_blocking().expect("load").expect("plan").revision,
        0
    );
}

#[test]
fn forged_snapshots_and_duplicate_operation_ids_are_atomic_failures() {
    let mut store = SqliteStore::in_memory_blocking().expect("store");
    let mut plan = fixture();
    let initial = plan.clone();
    store.initialize_blocking(&plan).expect("initialize");
    let op = claim(&mut plan, "owner");
    let mut forged = plan.clone();
    forged.workspace.name = "forged".into();
    assert!(matches!(
        store.persist_blocking(&forged, &op),
        Err(StoreError::SnapshotMismatch(_))
    ));
    assert_eq!(store.load_blocking().expect("load"), Some(initial));
    store.persist_blocking(&plan, &op).expect("persist");
    let stored = plan.clone();
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    let mut second = apply_command(
        &mut plan,
        ActorId::agent("owner"),
        Command::Start { work },
        Utc::now(),
    )
    .expect("start");
    second.id = op.id;
    assert!(matches!(
        store.persist_blocking(&plan, &second),
        Err(StoreError::Database { .. })
    ));
    assert_eq!(store.load_blocking().expect("load"), Some(stored));
    assert_eq!(store.operation_count_blocking().expect("count"), 1);
}

#[test]
fn snapshot_write_failure_rolls_back_the_operation_insert() {
    let mut store = SqliteStore::in_memory_blocking().expect("store");
    let mut plan = fixture();
    let initial = plan.clone();
    store.initialize_blocking(&plan).expect("initialize");
    store.connection.execute_batch("CREATE TEMP TRIGGER fail_snapshot BEFORE UPDATE ON plan_state BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;").expect("trigger");
    let op = claim(&mut plan, "owner");
    assert!(store.persist_blocking(&plan, &op).is_err());
    assert_eq!(store.load_blocking().expect("load"), Some(initial));
    assert_eq!(store.operation_count_blocking().expect("count"), 0);
}

#[test]
fn corrupt_revision_and_lifecycle_are_rejected_on_load() {
    let mut store = SqliteStore::in_memory_blocking().expect("store");
    let mut plan = fixture();
    store.initialize_blocking(&plan).expect("initialize");
    store
        .connection
        .execute("UPDATE plan_state SET revision = 9", [])
        .expect("corrupt metadata");
    assert!(matches!(
        store.load_blocking(),
        Err(StoreError::CorruptSnapshot { .. })
    ));
    plan.find_work_by_key_mut("TEST-A").expect("task").status = WorkStatus::Claimed;
    store
        .connection
        .execute(
            "UPDATE plan_state SET revision = 0, plan_json = ?1",
            [serde_json::to_string(&plan).expect("json")],
        )
        .expect("corrupt state");
    assert!(matches!(
        store.load_blocking(),
        Err(StoreError::Validation(_))
    ));
}

#[test]
fn stores_written_before_edge_identity_keep_history_and_accept_new_operations() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("legacy.sqlite");
    let legacy: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("legacy json");
    assert!(legacy["dependencies"][0].get("id").is_none());
    let mut renamed = legacy.clone();
    renamed["workspace"]["name"] = "Renamed".into();
    renamed["revision"] = 1.into();
    let change = serde_json::json!({"ApplyChange": {"plan": legacy, "reason": "rename"}});
    {
        let connection = Connection::open(&path).expect("raw connection");
        connection
            .execute_batch(&format!(
                "{}; {};",
                crate::schema::TEST_PLAN_STATE_DDL,
                crate::schema::TEST_OPERATIONS_DDL
            ))
            .expect("originless layout");
        connection
            .execute(
                "INSERT INTO plan_state(singleton, revision, plan_json) VALUES(1, 1, ?1)",
                [renamed.to_string()],
            )
            .expect("legacy snapshot");
        connection
            .execute(
                "INSERT INTO operations(operation_id, base_revision, resulting_revision, actor_json, timestamp, command_json)
                 VALUES('6f1c1a52-7d1e-4d57-9d53-0d3f7a1b2c3d', 0, 1, '{\"kind\":\"Human\",\"name\":\"lead\"}', '2026-01-01T00:00:00+00:00', ?1)",
                [change.to_string()],
            )
            .expect("legacy operation");
    }
    let mut store = SqliteStore::open_existing_blocking(&path).expect("reopen");
    let mut plan = store.load_blocking().expect("load").expect("plan");
    for edge in &plan.dependencies {
        let derived =
            dpm_model::Dependency::derived_id(edge.predecessor, edge.successor, edge.kind);
        assert_eq!(edge.id, derived);
    }
    let derived: Vec<String> = plan.dependencies.iter().map(|d| d.id.to_string()).collect();
    assert_eq!(
        store
            .history_blocking(0, 10)
            .expect("history")
            .entries
            .len(),
        1
    );
    let op = claim(&mut plan, "worker");
    store
        .persist_blocking(&plan, &op)
        .expect("persist on a legacy snapshot");
    drop(store);
    let reopened = SqliteStore::open_existing_blocking(&path).expect("reopen");
    let stored: String = reopened
        .connection
        .query_row("SELECT plan_json FROM plan_state", [], |row| row.get(0))
        .expect("snapshot");
    let stored: serde_json::Value = serde_json::from_str(&stored).expect("json");
    let stored_ids: Vec<String> = stored["dependencies"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|d| d["id"].as_str().expect("explicit id").to_string())
        .collect();
    assert_eq!(stored_ids, derived);
    assert_eq!(reopened.load_blocking().expect("load"), Some(plan));
    let history = reopened.history_blocking(0, 10).expect("history");
    assert_eq!(history.entries.len(), 2);
}
