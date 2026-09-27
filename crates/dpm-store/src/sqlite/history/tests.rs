use super::*;
use chrono::Utc;
use dpm_model::{ActorId, Plan};

#[test]
fn chronological_pages_preserve_operation_identity_across_revision_wrap() {
    let mut plan = Plan::empty("History");
    plan.revision = u64::MAX;
    let mut store = SqliteStore::in_memory_blocking().expect("store");
    store.initialize_blocking(&plan).expect("init");
    let mut ids = Vec::new();
    for name in ["First", "Second"] {
        let mut proposed = plan.clone();
        proposed.workspace.name = name.into();
        let operation = dpm_engine::apply_plan_change(
            &mut plan,
            ActorId::human("planner"),
            &proposed,
            "rename",
            Utc::now(),
            dpm_model::OperationId::new(),
        )
        .expect("apply");
        store.persist_blocking(&plan, &operation).expect("persist");
        ids.push(operation.id);
    }
    let first = store.history_blocking(0, 1).expect("first page");
    assert_eq!(first.revision, 1);
    assert_eq!(first.entries.len(), 1);
    assert_eq!(first.entries[0].operation.id, ids[0]);
    assert_eq!(first.entries[0].operation.base_revision, u64::MAX);
    assert_eq!(first.entries[0].operation.resulting_revision, 0);
    let second = store
        .history_blocking(first.next_after_sequence, 1)
        .expect("second");
    assert_eq!(second.entries[0].operation.id, ids[1]);
    assert!(
        store
            .history_blocking(second.next_after_sequence, 10)
            .expect("end")
            .entries
            .is_empty()
    );
    assert!(
        store
            .history_blocking(u64::MAX, 10)
            .expect("cursor beyond end")
            .entries
            .is_empty()
    );
    assert_eq!(store.load_blocking().expect("load"), Some(plan));
    assert_eq!(store.operation_count_blocking().expect("count"), 2);
}
