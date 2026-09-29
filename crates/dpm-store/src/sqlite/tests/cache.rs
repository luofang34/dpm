use super::*;

#[test]
fn cached_reads_see_external_writes_and_do_not_trust_revision_alone() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("plan.sqlite");
    let mut first = SqliteStore::open_blocking(&path).expect("store");
    let original = fixture();
    first.initialize_blocking(&original).expect("initialize");
    let mut returned = first.load_blocking().expect("load").expect("plan");
    returned.workspace.name = "caller edit".into();
    assert_eq!(
        first.load_blocking().expect("cached"),
        Some(original.clone())
    );
    let mut other = SqliteStore::open_existing_blocking(&path).expect("other");
    let mut updated = original.clone();
    let operation = claim(&mut updated, "other");
    other
        .persist_blocking(&updated, &operation, None)
        .expect("commit");
    assert_eq!(
        first.load_blocking().expect("refresh"),
        Some(updated.clone())
    );
    let connection = Connection::open(&path).expect("external");
    connection
        .execute("UPDATE plan_state SET snapshot_json = '{}'", [])
        .expect("corrupt");
    assert!(
        first.load_blocking().is_err(),
        "same revision must not hide corrupted bytes"
    );
    connection
        .execute(
            "UPDATE plan_state SET snapshot_json = ?1, revision = 2",
            [serde_json::to_string(&updated).expect("json")],
        )
        .expect("row mismatch");
    assert!(matches!(
        first.load_blocking(),
        Err(StoreError::CorruptSnapshot { .. })
    ));
    connection
        .execute("UPDATE plan_state SET revision = 1", [])
        .expect("repair row");
    assert_eq!(first.load_blocking().expect("repaired"), Some(updated));
}
